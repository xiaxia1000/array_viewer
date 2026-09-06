//! 可视化二维数组包装，集成数据缓冲区、显示缓冲区和渲染线程。

use crate::base::{ScreenArray, immutable::ScreenArrayBase};
use crate::unsafe_pointer::UnsafePointerHandler;
use crate::viewer::ArrayViewer;
use crate::viewer::exchange_layer::ExchangeLayer;
use num_traits::Zero;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

/// 可视化的二维数组，用于展示类型 `T` 的元素。
/// 设计上相当于自动管理数组和线程生命周期的包装，应视为是ScreenArray和ArrayViewer的语法糖。
///
/// # 泛型参数
/// - `T`: 数据像素类型，无特殊 trait 约束（只需 `Sized`）。
/// - `W`, `H`: 固定宽度和高度（编译期常量）。
///
/// # 懒加载：创建与启动分离
/// - [`new`](Self::new) 只分配数据缓冲区 `data` 并保存转换闭包，**不会**创建窗口。
/// - 调用 [`run`](Self::run) 时才创建显示缓冲区 `display` 与 `ArrayViewer`，
///   并启动显示线程（懒加载）。
///
/// # 线程模型
/// - `run` 启动后，显示线程每帧读取 `data`，经转换闭包写入 `display` 并渲染。
/// - 数据缓冲区 `data` 由主线程持有；并发读写数据需由调用者保证同步。
/// - `stop` / `drop` 时自动停止线程并释放所有堆内存。
pub struct VisualArray<T: 'static + Zero, const W: usize, const H: usize> {
    data: ScreenArrayBase<T, W, H>,
    display: Option<ScreenArray<W, H>>,
    viewer: Option<ArrayViewer<W, H>>,
    handle: Option<JoinHandle<()>>,
    converter: Option<Box<dyn Fn(&T) -> u32 + Send>>,
}

impl<T: 'static + Zero, const W: usize, const H: usize> VisualArray<T, W, H> {
    /// 使用初始二维数组和转换闭包创建可视数组。
    ///
    /// 内部会：
    /// 1. 创建数据缓冲区（全零）。
    /// 2. 保存转换闭包，供 `run` 后显示线程每帧调用。
    ///
    /// 注意：本方法**不会**启动窗口，需再调用 [`run`](Self::run)。
    pub fn new(converter: Box<dyn Fn(&T) -> u32 + Send>) -> Self {
        let data = ScreenArrayBase::zero();
        Self {
            data,
            display: None,
            viewer: None,
            handle: None,
            converter: Some(converter),
        }
    }

    /// 懒加载启动：创建显示缓冲区和 `ArrayViewer`，并启动显示线程。
    ///
    /// # 行为
    /// 1. 分配显示缓冲区（全零）。
    /// 2. 创建 `ArrayViewer` 指向显示缓冲区。
    /// 3. 把“data → display 转换”注册为显示线程每帧执行的更新钩子。
    /// 4. 启动显示线程。
    ///
    /// 若线程已在运行则幂等返回；同一实例在 `stop` 后无法再次 `run`
    /// （转换闭包已被消费），需要重建 `VisualArray`。
    ///
    /// # Panics
    /// - 当转换闭包已被消费（曾调用过 `run`）时 panic。
    /// - 若窗口创建失败（如不支持的分辨率）则会 panic。
    pub fn run(&mut self) {
        // 已在运行：幂等返回
        if self.handle.is_some() {
            return;
        }

        let converter = self.converter.take().expect(
            "[VisualArray::run] converter 已被使用；run() 只能调用一次，请重建 VisualArray",
        );

        // 1. 懒加载：此时才分配显示缓冲区并创建 viewer
        let display = ScreenArray::zero();
        let viewer = ArrayViewer::new(display.get_ptr() as usize);

        // 2. 注册每帧转换钩子。
        // data / display 均在堆上、地址固定，因此可捕获裸指针后放入 'static 闭包。
        let data_handler = unsafe { UnsafePointerHandler::from_mut_ptr(self.data.get_ptr()) };
        let display_handler = unsafe { UnsafePointerHandler::from_mut_ptr(display.get_ptr()) };

        viewer.get_exchange_layer().set_update_hook(move |_| {
            unsafe {
                // 按行分块转换复制，避免逐像素索引导致的 cache miss（见 convert_to_display）。
                convert_to_display(
                    data_handler.deref(),
                    display_handler.deref_mut(),
                    &*converter,
                );
            }
        });

        // 3. 启动显示线程，使用默认窗口选项
        self.handle = Some(viewer.run(None));
        self.display = Some(display);
        self.viewer = Some(viewer);
    }

    /// 获取数据的可变引用（由显示线程每帧读取并自动刷新到窗口）。
    ///
    /// 注意：修改后无需手动刷新——`run` 注册的钩子会在下一帧把 `data`
    /// 经转换闭包复制到 `display`。并发写读需由调用者保证同步。
    pub fn get_data_mut(&mut self) -> &mut ScreenArrayBase<T, W, H> {
        &mut self.data
    }

    /// 获取数据的不可变引用。
    pub fn get_data(&self) -> &ScreenArrayBase<T, W, H> {
        &self.data
    }

    /// 获取显示缓冲区的不可变引用（只读）。
    ///
    /// 由于创建与启动分离，在调用 [`run`](Self::run) 之前返回 `None`。
    pub fn get_display(&self) -> Option<&ScreenArray<W, H>> {
        self.display.as_ref()
    }

    /// 停止显示线程并释放渲染相关资源（数据缓冲区保留）。
    ///
    /// 可在 `drop` 前主动调用，以便控制退出时机。调用后无法再启动。
    pub fn stop(&mut self) {
        // 1. 若渲染线程仍在运行，请求退出并等待其结束
        if let Some(handle) = self.handle.take() {
            if let Some(ref viewer) = self.viewer {
                viewer
                    .get_exchange_layer()
                    .is_running
                    .store(false, Ordering::Relaxed);
            }
            let _ = handle.join(); // 忽略错误
        }

        // 2. 线程结束后清除钩子，释放其捕获的裸指针与转换闭包
        if let Some(ref viewer) = self.viewer {
            viewer.get_exchange_layer().clear_update_hook();
        }

        // 3. 释放显示缓冲区堆内存并移除 viewer
        if let Some(mut display) = self.display.take() {
            display.drop();
        }
        self.viewer = None;
    }

    /// 获取当前显示帧率（由渲染线程更新）。
    ///
    /// 若尚未调用 [`run`](Self::run) 或线程已结束，返回 0。
    pub fn fps(&self) -> usize {
        self.viewer.as_ref().map(|v| v.fps()).unwrap_or(0)
    }

    /// 获取交换层（`ExchangeLayer`）的共享引用，用于控制窗口属性、读取输入等。
    ///
    /// 由于创建与启动分离，在调用 [`run`](Self::run) 之前返回 `None`。
    pub fn exchange_layer(&self) -> Option<Arc<ExchangeLayer>> {
        self.viewer.as_ref().map(|v| v.get_exchange_layer())
    }
}

impl<T: 'static + Zero, const W: usize, const H: usize> Drop for VisualArray<T, W, H> {
    fn drop(&mut self) {
        // 1. 停止渲染线程并释放显示相关资源
        self.stop();

        // 2. 手动释放数据缓冲区堆内存
        self.data.drop();
    }
}

/// 按行优先分块把 `data` 经 `converter` 转换后写入 `display`。
///
/// # 设计动机（为何分块/逐行处理）
/// 直接在一维索引上做 `i / W`、`i % W` 映射会有两个问题：
/// - 每个像素都做除法/取模，开销大；
/// - 虽然源、目标本身是行优先连续的，但分散索引容易造成 cache miss。
///
/// 这里以**行**为最小分块单元：每一行在源与目标中都连续，读写均为顺序访问，
/// 利于 cache 命中；同时也为将来按更大 tile（如 8×8、16×16）扩展留出空间。
fn convert_to_display<T, const W: usize, const H: usize>(
    data: &[[T; W]; H],
    display: &mut [[u32; W]; H],
    converter: &dyn Fn(&T) -> u32,
) {
    for (src_row, dst_row) in data.iter().zip(display.iter_mut()) {
        for (src, dst) in src_row.iter().zip(dst_row.iter_mut()) {
            *dst = converter(src);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造转换闭包：把任意值映射为不透明灰色（仅保留低字节）。
    fn gray() -> Box<dyn Fn(&u32) -> u32 + Send> {
        Box::new(|v: &u32| 0xFF00_0000 | (v & 0x0000_00FF))
    }

    #[test]
    fn convert_to_display_copies_in_row_major() {
        const W: usize = 4;
        const H: usize = 3;

        // 源数据：3 行 × 4 列，值 = 1..=12（行优先）
        let data = [[1u32, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12]];
        let mut display = [[0u32; W]; H];

        convert_to_display(&data, &mut display, &*gray());

        // 逐行逐列验证 ARGB 低字节与原值一致
        for (y, src_row) in data.iter().enumerate() {
            for (x, src) in src_row.iter().enumerate() {
                assert_eq!(display[y][x], 0xFF00_0000 | (src & 0xFF));
            }
        }
    }

    #[test]
    fn convert_to_display_zero_alpha_kept() {
        // Alpha 位不应被转换闭包之外的其他逻辑篡改：验证转换只依赖闭包本身。
        let data = [[0xABCD_EF01u32]];
        let mut display = [[0u32; 1]];
        convert_to_display(&data, &mut display, &|v| *v);
        assert_eq!(display[0][0], 0xABCD_EF01);
    }

    #[test]
    fn new_does_not_start_rendering() {
        // 懒加载：new 之后不应有显示缓冲、viewer、渲染线程或窗口。
        let va = VisualArray::<u32, 8, 8>::new(gray());
        assert!(va.display.is_none());
        assert!(va.viewer.is_none());
        assert!(va.handle.is_none());
        assert!(va.converter.is_some());
        assert_eq!(va.fps(), 0);
        assert!(va.exchange_layer().is_none());
        assert!(va.get_display().is_none());
        assert_eq!(va.data.as_slice(), &[0u32; 8 * 8]);
    }
}
