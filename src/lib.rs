//! 提供二维像素缓冲区 (`ScreenArray`) 和窗口显示 (`ArrayViewer`) 的轻量级绑定。
//!
//! 该库的核心是分离数据所有权与显示逻辑：
//! - `ScreenArray` 管理堆上的像素内存，可跨线程移动。
//! - `ArrayViewer` 通过原始指针零拷贝渲染像素，在独立线程中运行。
//! - `ExchangeLayer` 负责主线程与显示线程之间的状态同步（输入、窗口属性等）。
//!
//! # 典型用法
//! ```no_run
//! use array_viewer::init;
//! let (mut arr, viewer) = init::<800, 600>();
//! let handle = viewer.run(None, None);
//! // 主线程通过 arr.as_mut_slice() 修改像素，通过 viewer.get_exchange_layer() 控制窗口
//! // ...
//! handle.join().unwrap();
//! arr.drop();  // 手动释放内存
//! ```

mod viewer;
mod array;
mod visual;

pub use minifb::Key;
pub use array::*;
pub use viewer::*;
pub use visual::*;
use crate::base::ScreenArray;

/// 创建一个新的屏幕缓冲区和对应的显示实例，并返回它们的元组。
///
/// 该函数是使用默认配置快速启动的入口：
/// - `ScreenArray` 被初始化为全零（黑色，ARGB 格式）。
/// - `ArrayViewer` 将持有该缓冲区的原始指针，并在随后启动的线程中渲染。
///
/// # 类型参数
/// - `W`: 屏幕宽度（像素数）。
/// - `H`: 屏幕高度（像素数）。
///
/// # 返回值
/// - 第一个元素是可变的 `ScreenArray<W, H>`，用于修改像素数据。
/// - 第二个元素是 `ArrayViewer<W, H>`，用于启动显示线程和控制窗口。
///
/// # 示例
/// ```no_run
/// use array_viewer::init;
/// let (arr, viewer) = init::<800, 600>();
/// // 填充像素...
/// let handle = viewer.run(None, None);
/// // ...
/// ```
pub fn init<const W: usize, const H: usize>() -> (ScreenArray<W, H>, ArrayViewer<W, H>) {
    let arr = ScreenArray::zero();
    let view = ArrayViewer::new(arr.get_ptr() as usize);
    (arr, view)
}



#[cfg(test)]
mod test {
    use crate::init;
    use crate::viewer::ArrayViewer;
    use noise::{NoiseFn, SuperSimplex};
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    const WIDTH: usize = 320;
    const HEIGHT: usize = 240;

    /// 测试：启动显示线程渲染有限帧后正常退出。
    ///
    /// 验证“主线程写像素 + 显示线程刷新”的整条流水线可以跑通。
    #[test]
    fn default() {
        let mut test_data = vec![0u32; WIDTH * HEIGHT];

        // 创建显示实例并启动线程
        let display = ArrayViewer::<WIDTH, HEIGHT>::new(test_data.as_ptr() as usize);
        let exchange_layer = display.get_exchange_layer();
        let running_flag = &exchange_layer.is_running;
        let handle = display.run(None, None);

        // 噪声参数
        const A: f64 = 0.003;
        const MAX_FRAMES: u32 = 6000;
        let mut frame = 0;
        let noise = SuperSimplex::default();

        // 主线程持续更新像素数据，直到显示线程通知停止或达到帧数上限
        while running_flag.load(Ordering::Relaxed) && frame < MAX_FRAMES {
            frame += 1;
            let t = frame as f64 * A * 10.0;

            test_data
                .iter_mut()
                .enumerate()
                .for_each(|(idx, color)| {
                    let w = (idx % WIDTH) as f64 * A;
                    let h = (idx / WIDTH) as f64 * A;

                    let r = ((noise.get([h, w, t]) + 1.0) * 127.5) as u32;
                    let g = ((noise.get([h + 10.0, w + 10.0, t + 10.0]) + 1.0) * 127.5) as u32;
                    let b = ((noise.get([h + 20.0, w + 20.0, t + 20.0]) + 1.0) * 127.5) as u32;

                    *color = (0xFF << 24) | (r << 16) | (g << 8) | b;
                });
        }

        // 请求退出并等待显示线程结束
        running_flag.store(false, Ordering::Relaxed);
        handle.join().unwrap();
    }

    /// 画笔测试：用新的键盘/鼠标状态 API 驱动绘图循环。
    ///
    /// 运行有限帧后自动退出，验证 `get_key_state()` / `get_mouse_state()`
    /// 与 `update_signal` / `apply_signal` 的配合。
    #[test]
    fn paint_test() {
        use minifb::Key;

        const W: usize = 320;
        const H: usize = 240;
        const MAX_FRAMES: u32 = 6000;

        // 1. 创建像素缓冲区和显示实例
        let (mut arr, viewer) = init::<W, H>();
        let exchange = viewer.get_exchange_layer();

        // 2. 启动显示线程
        let handle = viewer.run(None, None);

        // 等待线程真正开始运行
        while !exchange.is_running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
        }

        // 初始画笔颜色（红色，ARGB）
        let mut color = 0xFFFF_0000u32;
        // 画笔半径
        let mut brush_radius = 10;
        // 是否启用圆形画笔
        let mut circle_brush = true;

        // 每帧的键盘/鼠标状态视图（借用 ExchangeLayer 内部的脏位图）
        let mut key_state = exchange.get_key_state();
        let mut mouse_state = exchange.get_mouse_state();

        // 3. 主循环：持续处理输入并更新像素（有限帧，自动退出）
        let mut frames = 0u32;
        while exchange.is_running.load(Ordering::Relaxed) && frames < MAX_FRAMES {
            mouse_state.update();
            key_state.update();

            frames += 1;

            // ESC 退出
            if key_state.is_down(Key::Escape) {
                exchange.is_running.store(false, Ordering::Relaxed);
                break;
            }

            // 鼠标位置（窗口坐标系）
            let mx = exchange.mouse_pos.0.get();
            let my = exchange.mouse_pos.1.get();
            let (x, y) = (mx as usize, my as usize);

            // 鼠标在窗口内时处理绘图
            if x < W && y < H {
                let left_pressed = mouse_state.is_left_pressed();
                let right_pressed = mouse_state.is_right_pressed();

                if left_pressed || right_pressed {
                    let slice = arr.as_mut_slice();
                    let draw_color = if right_pressed { 0xFF00_0000 } else { color };

                    if circle_brush {
                        // 圆形画笔：绘制实心圆
                        draw_circle(slice, W, H, x, y, brush_radius, draw_color);
                    } else {
                        // 点画笔：只绘制单个像素
                        let idx = y * W + x;
                        slice[idx] = draw_color;
                    }
                }
            }

            // 空格键切换颜色
            if key_state.is_down(Key::Space) {
                color = match color {
                    0xFFFF_0000 => 0xFF00_FF00, // 红 → 绿
                    0xFF00_FF00 => 0xFF00_00FF, // 绿 → 蓝
                    _ => 0xFFFF_0000,           // 蓝 → 红
                };
            }

            // C 键清屏
            if key_state.is_down(Key::C) {
                arr.as_mut_slice().fill(0xFF00_0000);
            }

            // +/- 调整画笔大小
            if key_state.is_down(Key::Equal) {  // + 键
                brush_radius = (brush_radius + 2).min(50);
            }
            if key_state.is_down(Key::Minus) {  // - 键
                brush_radius = (brush_radius - 2).max(2);
            }

            // R 键切换画笔模式
            if key_state.is_down(Key::R) {
                circle_brush = !circle_brush;
            }

            // --- 状态显示（窗口标题）---
            let title = format!(
                "画笔 | 坐标({:.0},{:.0}) | 半径:{} | 模式:{} | FPS:{} | 帧:{}",
                mx,
                my,
                brush_radius,
                if circle_brush { "圆形" } else { "点" },
                viewer.fps(),
                frames,
            );
            {
                let mut lock = exchange.title.lock().unwrap();
                *lock = Some(title);
            }



            // 控制循环速度，避免 CPU 满载
            std::thread::sleep(Duration::from_millis(10));
        }

        // 4. 请求退出，等待显示线程结束并释放内存
        exchange.is_running.store(false, Ordering::Relaxed);
        handle.join().unwrap();
        arr.drop();

        /// 绘制实心圆
        fn draw_circle(slice: &mut [u32], width: usize, height: usize,
                       cx: usize, cy: usize, radius: usize, color: u32) {
            // 计算圆的边界
            let x_start = if cx > radius { cx - radius } else { 0 };
            let x_end = (cx + radius + 1).min(width);
            let y_start = if cy > radius { cy - radius } else { 0 };
            let y_end = (cy + radius + 1).min(height);

            let radius_sq = (radius * radius) as f32;

            for y in y_start..y_end {
                for x in x_start..x_end {
                    // 计算当前像素到圆心的距离平方
                    let dx = (x as isize - cx as isize) as f32;
                    let dy = (y as isize - cy as isize) as f32;
                    let dist_sq = dx * dx + dy * dy;

                    // 如果距离小于等于半径，绘制该像素
                    if dist_sq <= radius_sq {
                        let idx = y * width + x;
                        slice[idx] = color;
                    }
                }
            }
        }
    }
}