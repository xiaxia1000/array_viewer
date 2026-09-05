//! 显示模块，负责在独立线程中渲染像素缓冲区到窗口。

pub(crate) mod exchange_layer;

use crate::viewer::exchange_layer::ExchangeLayer;
use chrono::Local;
use minifb::{Window, WindowOptions};
#[cfg(feature = "view_shot")]
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

/// 显示结构体，持有像素缓冲区的原始指针、尺寸以及运行标志。
///
/// # Safety
/// - 指针 `ptr` 必须指向至少 `width * height` 个 `u32` 的有效内存区域。
/// - 在 `ArrayViewer` 存续期间，该内存区域必须保持有效且不会被释放或移动。
///
/// # 功能概述
/// - 通过 `get_exchange_layer()` 获取 `ExchangeLayer`，用于控制窗口属性、读取输入等。
/// - `run()` 启动一个独立线程，持续刷新显示画面。
/// - `fps()` 可获取当前显示线程的实际帧率（每秒帧数）。
/// - 启用 `view_shot` feature 后可使用 `view_shot()` 保存当前画面为图片。
///
/// # 线程模型
/// `ArrayViewer` 本身可以在任意线程创建，但 `run()` 会生成一个新的显示线程。
/// 所有与窗口的交互（包括输入、标题更新等）都通过 `ExchangeLayer` 进行跨线程通信。
pub struct ArrayViewer<const W: usize, const H: usize> {
    ptr: usize,
    exchange_layer: Arc<ExchangeLayer>,
}

impl<const W: usize, const H: usize> ArrayViewer<W, H> {
    /// 创建一个新的显示实例。
    ///
    /// 默认配置：
    /// - 窗口标题：`"starting..."`（启动后由 `ExchangeLayer` 控制）。
    /// - 目标帧率：60 FPS（可通过 `ExchangeLayer.target_fps` 修改）。
    /// - 退出按键：默认由窗口关闭按钮控制，也可通过 `ExchangeLayer.is_running` 控制。
    /// - 窗口选项：`WindowOptions::default()`（可在 `run()` 时覆盖）。
    ///
    /// # Safety
    /// 调用者必须保证指针有效性，参见结构体文档。
    pub fn new(ptr: usize) -> Self {
        Self {
            ptr,
            exchange_layer: Arc::from(ExchangeLayer::new()),
        }
    }

    // TODO: 把update_hook的闭包放到exchang_layer中，由Mutex持有，渲染线程应有一个hook的缓存用于在try_lock失败时复用
    /// 启动显示线程，返回线程句柄。
    ///
    /// 该方法不会消耗 `self`，因此可以在启动后继续调用 `fps()` 或通过 `exchange_layer` 控制。
    ///
    /// 线程内部逻辑：
    /// 1. 根据配置创建 [`Window`]。
    /// 2. 设置目标帧率（若有）。
    /// 3. 循环执行 `update_with_buffer` 将像素缓冲区刷新到窗口。
    /// 4. 在每帧开始和结束时，从 `ExchangeLayer` 应用配置（如标题、位置等），
    ///    并将窗口状态（鼠标、键盘等）更新回 `ExchangeLayer`。
    /// 5. 当窗口关闭或 `ExchangeLayer.is_running` 被置为 `false` 时退出循环。
    ///
    /// # 参数
    /// - `window_options`：可选的窗口创建选项，若为 `None` 则使用默认值。
    ///
    /// # 返回值
    /// 线程的 `JoinHandle`，可用于等待线程结束。
    pub fn run(
        &self,
        window_options: Option<WindowOptions>,
        update_hook: Option<Box<dyn Fn(Arc<ExchangeLayer>) + Send>>) -> JoinHandle<()>
    {
        let exchange_layer = self.exchange_layer.clone();
        let ptr = self.ptr;

        thread::spawn(move || {
            let running = &exchange_layer.is_running;
            let target_fps = &exchange_layer.target_fps;
            let fps = &exchange_layer.fps;

            let ptr = ptr as *const u32;
            // 从原始指针构造切片（只读，零拷贝）
            let pixels = unsafe { std::slice::from_raw_parts(ptr, W * H) };

            let mut window =
                Window::new("starting...", W, H, window_options.unwrap_or(WindowOptions::default()))
                    .expect("无法创建窗口");

            window.set_target_fps(target_fps.load(Ordering::Relaxed));

            // FPS 计算闭包（基于两次调用之间的时间差）
            let mut time = Local::now();
            let mut get_fps = move || -> usize {
                let now = Local::now();
                let interval = now - time;
                time = now;
                let nanos = interval.num_nanoseconds().unwrap_or(i64::MAX);
                (1_000_000_000 / nanos) as usize
            };

            // 设置运行标志位
            running.store(true, Ordering::Relaxed);

            // 主循环
            while window.is_open() && running.load(Ordering::Relaxed) {
                // 每帧更新的闭包
                if let Some(ref uh) = update_hook {
                    (*uh)(exchange_layer.clone());
                }

                // 应用主线程请求的窗口属性
                exchange_layer.apply_to_window(&mut window);

                // 刷新像素数据到窗口
                window
                    .update_with_buffer(pixels, W, H)
                    .expect("窗口更新失败");

                // 更新帧率统计
                let current_fps = get_fps();
                fps.store(current_fps, Ordering::Relaxed);

                // 从窗口读取输入状态并更新到 ExchangeLayer
                exchange_layer.update_from_window(&mut window);
            }

            // 通知主线程停止更新
            running.store(false, Ordering::Relaxed);
        })
    }

    /// 返回当前显示线程的实时帧率。
    ///
    /// 该值由显示线程每次刷新后更新，可能略有延迟。
    /// 若线程尚未启动或已结束，返回 0。
    pub fn fps(&self) -> usize  {
        (&self.exchange_layer.fps).load(Ordering::Relaxed)
    }

    /// 获取 `ExchangeLayer` 的共享引用，用于跨线程控制窗口。
    pub fn get_exchange_layer(&self) -> Arc<ExchangeLayer> { self.exchange_layer.clone() }

    #[cfg(feature = "view_shot")]
    /// 将当前像素缓冲区保存为图片文件（例如 PNG、JPEG 等）。
    ///
    /// 路径由调用者指定，格式由文件扩展名决定（需 `image` crate 支持）。
    ///
    /// # 示例
    /// ```no_run
    /// use array_viewer::ArrayViewer;
    /// let array = [[333u32; 64]; 48];
    /// let ptr = Box::into_raw(Box::new(array)) as usize;
    /// let viewer = ArrayViewer::<64, 48>::new(ptr);
    /// assert!(viewer.view_shot("screenshot.png").is_ok());
    /// ```
    pub fn view_shot<P: AsRef<Path>>(&self, path: P) -> Result<(), Box<dyn std::error::Error>> {
        // 从原始指针构造只读切片（零拷贝）
        let pixels = unsafe { std::slice::from_raw_parts(self.ptr as *const u32, W * H) };

        // 创建 RGBA 图像缓冲区
        let mut img = image::RgbaImage::new(W as u32, H as u32);

        for (i, &pixel) in pixels.iter().enumerate() {
            let x = (i % W) as u32;
            let y = (i / W) as u32;
            // minifb 使用 0xAARRGGBB 格式
            let a = ((pixel >> 24) & 0xFF) as u8;
            let r = ((pixel >> 16) & 0xFF) as u8;
            let g = ((pixel >> 8) & 0xFF) as u8;
            let b = (pixel & 0xFF) as u8;
            img.put_pixel(x, y, image::Rgba([r, g, b, a]));
        }

        // 保存图片（格式由扩展名推断）
        img.save(path)?;
        Ok(())
    }
}