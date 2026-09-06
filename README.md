# array_viewer

**Lightweight 2D pixel-buffer + window-viewer for Rust.**
**轻量级的 Rust 二维像素缓冲 + 窗口查看库。**

`array_viewer` separates pixel-data ownership from display logic:

- `ScreenArray` owns heap pixel memory (`u32`, ARGB) and can move across threads.
- `ArrayViewer` renders those pixels zero-copy (raw pointer) on its own background thread.
- `ExchangeLayer` keeps the main thread and the render thread in sync (window attrs, input, …).
- `VisualArray` is a sugar wrapper that owns both buffers and threads automatically.

`array_viewer` 的核心设计是**数据所有权与显示逻辑分离**：

- `ScreenArray` 在堆上持有像素内存（`u32`，ARGB），可跨线程移动。
- `ArrayViewer` 通过原始指针**零拷贝**渲染像素，渲染跑在独立线程中。
- `ExchangeLayer` 负责主线程与显示线程之间的状态同步（窗口属性、输入等）。
- `VisualArray` 是同时自动管理缓冲区与线程生命周期的语法糖包装。

---

## Features · 特性

- Fixed-size compile-time resolution (`W × H` const generics).
- Zero-copy rendering: the render thread reads your buffer directly.
- Background render thread with an `ExchangeLayer` for cross-thread control:
  window title / position / topmost / cursor, key & mouse states, scroll-wheel travel.
- Per-frame **update hooks** (`Fn(&mut Window)`) that can be set/replaced/cleared at runtime.
- Optional features: window screenshot (`view_shot`), image loading (`array_from_image`).
- Fixed-size 编译期分辨率（`W × H` const 泛型）。
- 零拷贝渲染：渲染线程直接读取你的像素缓冲。
- 后台渲染线程 + `ExchangeLayer` 跨线程控制：窗口标题 / 位置 / 置顶 / 光标、
  键盘与鼠标状态、滚轮行程等。
- 逐帧**更新钩子**（`Fn(&mut Window)`），运行时可设置 / 替换 / 清除。
- 可选特性：窗口截图（`view_shot`）、图片加载（`array_from_image`）。

---

## Quick Start · 快速开始

Add the dependency (and optional features you need):

```toml
[dependencies]
array_viewer = "0.1"

# 可选特性
# array_viewer = { version = "0.1", features = ["view_shot", "array_from_image"] }
```

### Low-level: `ScreenArray` + `ArrayViewer` · 底层用法

```rust,no_run
use array_viewer::init;

fn main() {
    let (mut arr, viewer) = init::<320, 240>();

    // 填充一整块颜色（ARGB）
    arr.as_mut_slice().fill(0xFF11_2233);

    // 启动显示线程（默认窗口选项）
    let handle = viewer.run(None);

    // 运行期间可通过 viewer.get_exchange_layer() 控制窗口 / 读取输入
    // viewer.get_exchange_layer().title.lock().unwrap() = Some("Hello".into());

    // 等待用户关闭窗口（渲染线程结束）
    handle.join().unwrap();

    // 手动释放堆内存
    arr.drop();
}
```

> 注意：`ScreenArray` 拥有堆内存但**不实现 `Drop`**，用完需手动 `arr.drop()`，
> 且不能 double-free。渲染期间若要在主线程写像素，请自行保证同步。

### High-level: `VisualArray` · 高层包装

```rust,no_run
use array_viewer::VisualArray;

fn main() {
    // 数据像素类型 T 可为任意类型，只需一个“T → u32(ARGB)”转换闭包
    let mut vis = VisualArray::<u32, 320, 240>::new(Box::new(|px: &u32| *px));

    // run() 之前即可修改数据（此时尚未创建窗口）
    vis.get_data_mut().as_mut_slice().fill(0xFF00_00FF); // 蓝色背景

    // 懒加载：此刻才创建窗口并启动渲染线程
    vis.run();

    // 显示线程每帧自动把 data 经转换闭包刷新到窗口
    std::thread::sleep(std::time::Duration::from_secs(3));

    vis.stop(); // 停止线程并释放渲染相关资源
}
```

---

## Feature Flags · 特性开关

| Feature | Description · 说明 | Default |
| --- | --- | --- |
| `view_shot` | Save the current window buffer to an image (`ArrayViewer::view_shot`) · 把当前画面保存为图片 | off |
| `array_from_image` | Load images into buffers from paths / `DynamicImage` · 从图片路径 / `DynamicImage` 载入缓冲 | off |

---

## Architecture & Key Types · 架构与核心类型

- **`ScreenArray<W, H>`** — a handle to fixed heap memory of `W × H` pixels (`u32`, ARGB);
  pointer address stays fixed until manual `drop()`. `ScreenArrayBase<T, W, H>` is the generic
  version for arbitrary pixel types.
  `ScreenArray<W, H>` —— 指向 `W × H` 个像素（`u32`，ARGB）堆内存的句柄，地址固定直到手动
  `drop()`；泛型版本为 `ScreenArrayBase<T, W, H>`，可用于任意像素类型。

- **`ArrayViewer<W, H>`** — renders a buffer on a background thread via minifb; created from a raw
  pointer. `run()` starts the thread, `fps()` reports the real frame rate.
  `ArrayViewer<W, H>` —— 用 minifb 在后台线程渲染缓冲，由裸指针创建。`run()` 启动线程，
  `fps()` 返回真实帧率。

- **`ExchangeLayer`** — shared state between threads: window size / position / title / topmost /
  cursor style & visibility, mouse position & buttons, scroll-wheel, key state, target & actual FPS.
  Use `get_key_state()` / `get_mouse_state()` / `get_scroll_wheel()` to read input frame by frame.
  `ExchangeLayer` —— 线程间共享状态：窗口尺寸 / 位置 / 标题 / 置顶 / 光标样式与可见性、
  鼠标位置与按键、滚轮、键盘状态、目标与实际帧率。用 `get_key_state()` / `get_mouse_state()` /
  `get_scroll_wheel()` 逐帧读取输入。

- **`VisualArray<T, W, H>`** — owns `data` and `display` buffers plus the viewer; converts
  `T`-typed data to ARGB each frame via a closure. Creation (`new`) and rendering (`run`) are
  separated (lazy window creation).
  `VisualArray<T, W, H>` —— 同时持有 `data`、`display` 缓冲与 viewer；每帧用闭包把 `T` 数据转成
  ARGB。创建（`new`）与渲染（`run`）分离（窗口懒加载）。

---

## Safety & Memory · 安全与内存

- `ScreenArrayBase` uses raw pointers and manual memory management on purpose
  (stable addresses for framebuffer / GPU / FFI use cases). Callers must respect the
  documented contracts: no double-free, no use-after-drop, coordinates in bounds.
- Data written by the main thread while the render thread reads it needs external
  synchronization (this crate is intentionally minimal here).
- `ScreenArrayBase` 出于 framebuffer / GPU / FFI 等场景的需要，使用裸指针 + 手动内存管理。
  调用者必须遵守文档契约：不可 double-free、释放后不可再访问、坐标不得越界。
- 渲染线程读取数据期间，主线程写入数据需要自行做外部同步（该库此处刻意保持最小化）。

---

## License · 许可

Dual-licensed under either

- MIT License ([text](http://opensource.org/licenses/MIT))
- Apache License, Version 2.0 ([text](http://www.apache.org/licenses/LICENSE-2.0))

at your option.

`SPDX-License-Identifier: MIT OR Apache-2.0`

