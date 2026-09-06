//! 可视化二维数组包装，集成数据缓冲区、显示缓冲区和渲染线程。

use crate::base::{ScreenArray, immutable::ScreenArrayBase};
use crate::viewer::ArrayViewer;
use crate::viewer::exchange_layer::ExchangeLayer;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread::JoinHandle;
use num_traits::Zero;
use crate::conversion::flatten;
use crate::unsafe_pointer::UnsafePointerHandler;

/// 可视化的二维数组，用于展示类型 `T` 的元素。
/// 设计上相当于自动管理数组和线程生命周期的包装，应视为是ScreenArray和ArrayViewer的语法糖
///
/// # 泛型参数
/// - `T`: 数据像素类型，无特殊 trait 约束（只需 `Sized`）。
/// - `W`, `H`: 固定宽度和高度（编译期常量）。
///
/// # 线程模型
/// - 构造时自动启动一个显示线程，独立渲染 `display` 缓冲区。
/// - 数据缓冲区 `data` 由主线程独占，修改后需调用 `update_display()` 刷新。
/// - `drop` 时自动停止线程并释放所有堆内存。
pub struct VisualArray<T: 'static + Zero, const W: usize, const H: usize> {
    data: ScreenArrayBase<T, W, H>,
    display: ScreenArray<W, H>,
    viewer: ArrayViewer<W, H>,
    handle: Option<JoinHandle<()>>,
}

impl<T: 'static + Zero, const W: usize, const H: usize> VisualArray<T, W, H> {
    /// 使用初始二维数组和转换闭包创建可视数组。
    ///
    /// 内部会：
    /// 1. 创建数据缓冲区并复制初始数据。
    /// 2. 创建显示缓冲区（全零，稍后刷新）。
    /// 3. 启动显示线程，指向显示缓冲区。
    ///
    /// # Panics
    /// 若窗口创建失败（如不支持的分辨率）则会 panic。
    pub fn new(converter: Box<dyn Fn(&T) -> u32 + Send>) -> Self {
        let data = ScreenArrayBase::zero();
        let display = ScreenArray::zero();
        let viewer = ArrayViewer::new(display.get_ptr() as usize);

        // TODO: 需要更优雅地实现，以下只是临时的。需要分Tile处理再拷贝以防止过多cache miss，
        // TODO: 需要完善['ArrayViewer']中的闭包调用与替换规则以获得良好的拓展性与约束，
        // TODO: 需要把新建结构体和启动渲染分离（显示缓冲区和viewer在调用运行窗口的方法是才创建【懒加载】）
        let data_slice = unsafe {
            UnsafePointerHandler::from_mut_ptr(data.get_ptr())
        };
        let display_slice = unsafe {
            UnsafePointerHandler::from_mut_ptr(display.get_ptr())
        };
        let handle = Some(viewer.run(
            None,
            Some(Box::new(move |_| {
                unsafe {
                    for (i, val) in flatten(data_slice.deref()).iter().enumerate() {
                        display_slice.deref_mut()[i / W][i % W] = converter(val);
                    }
                }
            })),
        )); // 使用默认窗口选项

        Self { data, display, viewer, handle, }
    }

    /// 获取数据的可变引用（需手动调用 `update_display` 才能刷新）。
    ///
    /// 注意：修改后必须显式调用 `update_display()`，否则显示不变。
    pub fn get_data_mut(&mut self) -> &mut ScreenArrayBase<T, W, H> {
        &mut self.data
    }

    /// 获取数据的不可变引用。
    pub fn get_data(&self) -> &ScreenArrayBase<T, W, H> {
        &self.data
    }

    /// 获取显示缓冲区的不可变引用（只读）。
    pub fn get_display(&self) -> &ScreenArray<W, H> {
        &self.display
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

impl<T: 'static + Zero, const W: usize, const H: usize> Drop for VisualArray<T, W, H> {
    fn drop(&mut self) {
        // 1. 停止渲染线程
        self.stop();

        // 2. 手动释放堆内存
        self.display.drop();
        self.data.drop();
        // viewer 自动释放，其内部 Arc<ExchangeLayer> 也会随之释放
    }
}