//! 可视化二维数组包装，集成数据缓冲区、显示缓冲区和渲染线程。

use crate::base::{ScreenArray, immutable::ScreenArrayBase};
use crate::viewer::ArrayViewer;
use crate::viewer::exchange_layer::ExchangeLayer;
use std::mem::ManuallyDrop;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;

/// 可视化的二维数组，用于展示类型 `T` 的元素。
///
/// # 泛型参数
/// - `T`: 数据像素类型，无特殊 trait 约束（只需 `Sized`）。
/// - `W`, `H`: 固定宽度和高度（编译期常量）。
/// - `F`: 转换闭包类型，`Fn(&T) -> u32`，将数据像素转换为 ARGB 颜色值。
///
/// # 线程模型
/// - 构造时自动启动一个显示线程，独立渲染 `display` 缓冲区。
/// - 数据缓冲区 `data` 由主线程独占，修改后需调用 `update_display()` 刷新。
/// - `drop` 时自动停止线程并释放所有堆内存。
pub struct VisualArray<T, const W: usize, const H: usize> {
    data: ManuallyDrop<ScreenArrayBase<T, W, H>>,
    display: ManuallyDrop<ScreenArray<W, H>>,
    viewer: ArrayViewer<W, H>,
    converter: Box<dyn Fn(&T) -> u32>,
    handle: Option<JoinHandle<()>>,
}

impl<T, const W: usize, const H: usize> VisualArray<T, W, H> {
    /// 使用初始二维数组和转换闭包创建可视数组。
    ///
    /// 内部会：
    /// 1. 创建数据缓冲区并复制初始数据。
    /// 2. 创建显示缓冲区（全零，稍后刷新）。
    /// 3. 启动显示线程，指向显示缓冲区。
    /// 4. 调用 `update_display()` 初始化显示内容。
    ///
    /// # Panics
    /// 若窗口创建失败（如不支持的分辨率）则会 panic。
    pub fn new(data: [[T; W]; H], converter: Box<dyn Fn(&T) -> u32>) -> Self {
        let data = ScreenArrayBase::new(data);
        let display = ScreenArray::zero();
        let viewer = ArrayViewer::new(display.get_ptr() as usize);
        let handle = Some(viewer.run(None)); // 使用默认窗口选项

        let this = Self {
            data: ManuallyDrop::new(data),
            display: ManuallyDrop::new(display),
            viewer,
            converter,
            handle,
        };
        this.update_display();
        this
    }

    /// 更新显示缓冲区：遍历数据，应用转换闭包，填充显示缓冲区。
    pub fn update_display(&self) {
        let data_slice = self.data.as_slice();
        let display_slice = self.display.as_mut_slice();
        for (i, val) in data_slice.iter().enumerate() {
            display_slice[i] = (self.converter)(val);
        }
    }

    /// 通过闭包修改数据并自动刷新显示。
    ///
    /// 这是推荐的数据修改方式，能保证显示与数据一致。
    ///
    /// # 示例
    /// ```no_run
    /// # use array_viewer::visual::VisualArray;
    /// # let mut vis = VisualArray::<u8, 10, 10, _>::new([[0;10];10], |&v| v as u32);
    /// vis.modify_data(|data| {
    ///     // 例如：将左上角像素设为 255
    ///     unsafe { *data.get_from_index_mut(0,0) = 255; }
    /// });
    /// ```
    pub fn modify_data<G>(&mut self, f: G)
    where
        G: FnOnce(&mut ScreenArrayBase<T, W, H>),
    {
        f(&mut *self.data);
        self.update_display();
    }

    /// 获取数据的可变引用（需手动调用 `update_display` 才能刷新）。
    ///
    /// 注意：修改后必须显式调用 `update_display()`，否则显示不变。
    pub fn get_data_mut(&mut self) -> &mut ScreenArrayBase<T, W, H> {
        &mut *self.data
    }

    /// 获取数据的不可变引用。
    pub fn get_data(&self) -> &ScreenArrayBase<T, W, H> {
        &*self.data
    }

    /// 获取显示缓冲区的不可变引用（只读）。
    pub fn get_display(&self) -> &ScreenArray<W, H> {
        &*self.display
    }

    /// 停止显示线程（但数据保留）。
    ///
    /// 可在 `drop` 前主动调用，以便控制退出时机。调用后无法再启动。
    pub fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            self.viewer
                .get_exchange_layer()
                .is_running
                .store(false, Ordering::Relaxed);
            let _ = handle.join(); // 忽略错误
        }
    }

    /// 获取当前显示帧率（由渲染线程更新）。
    pub fn fps(&self) -> usize {
        self.viewer.fps()
    }

    /// 获取交换层（`ExchangeLayer`）的共享引用，用于控制窗口属性、读取输入等。
    pub fn exchange_layer(&self) -> Arc<ExchangeLayer> {
        self.viewer.get_exchange_layer()
    }
}

impl<T, const W: usize, const H: usize> Drop for VisualArray<T, W, H> {
    fn drop(&mut self) {
        // 1. 停止渲染线程
        self.stop();

        // 2. 手动释放堆内存（必须在使用 ManuallyDrop 时手动 drop）
        unsafe {
            ManuallyDrop::drop(&mut self.display);
            ManuallyDrop::drop(&mut self.data);
        }
        // viewer 自动释放，其内部 Arc<ExchangeLayer> 也会随之释放
    }
}