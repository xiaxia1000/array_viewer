//! 定义主线程与显示线程之间的数据交换层。

use std::any::type_name;
use crate::viewer::exchange_layer::key_state::{KeyState, KeyStateDirtyMap};
use crate::viewer::exchange_layer::mouse_state::{MouseState, MouseStateDirtyMap};
use crate::viewer::exchange_layer::passed_f32::AtomicF32;
use minifb::{CursorStyle, MouseButton, MouseMode, Window};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use crate::viewer::exchange_layer::signal::{ApplyFlag, ApplySignal, ApplySignalNonAtomic, UpdateFlag, UpdateSignal, UpdateSignalNonAtomic};

mod hook;
pub(crate) use hook::SharedUpdateHook;

pub mod key_state;
pub mod passed_f32;
pub mod mouse_state;
pub mod signal;


/// 用于在主线程和显示线程之间交换状态的结构体。
///
/// 该结构体包含：
/// - 原子变量：窗口尺寸、位置、帧率、鼠标坐标等。
/// - 互斥锁保护的字段：窗口标题、光标样式。
/// - 原子信号：`update_signal` 和 `apply_signal`，用于控制哪些字段需要同步。
///
/// # 并发模型
/// 该结构体被设计为在线程间共享（通常使用 `Arc<ExchangeLayer>`）。
/// 显示线程定期调用 `update_from_window` 和 `apply_to_window` 来同步状态。
/// 主线程可以随时修改原子字段或设置信号，以请求显示线程应用更改。
///
/// # 注意
/// 信号机制用于减少不必要的操作：主线程设置信号，显示线程在每帧检查并执行。
#[derive(Debug, Default)]
pub struct ExchangeLayer {
    /// 更新信号：指示显示线程应从窗口读取哪些状态（输入、位置等）。
    pub update_signal: UpdateSignal,
    /// 应用信号：指示显示线程应将哪些配置应用到窗口（标题、位置、光标等）。
    pub apply_signal: ApplySignal,

    /// 窗口尺寸（宽, 高），由显示线程更新。
    pub window_size: (AtomicUsize, AtomicUsize),
    /// 窗口位置（x, y），由显示线程更新，也可由主线程设置并触发应用。
    pub window_position: (AtomicIsize, AtomicIsize),
    /// 窗口标题（互斥锁），由主线程设置，显示线程应用。
    pub title: Mutex<Option<String>>,
    /// 窗口是否置顶（原子布尔）。
    pub topmost: AtomicBool,
    /// 背景颜色（ARGB），仅当窗口未更新像素时显示。
    pub background_color: AtomicU32,
    /// 光标可见性。
    pub cursor_visibility: AtomicBool,
    /// 目标帧率（0 表示无限制），由显示线程读取。
    pub target_fps: AtomicUsize,
    /// 实际帧率（由显示线程更新，主线程只读）。
    pub fps: AtomicUsize,
    /// 鼠标位置（未缩放，窗口坐标系）。
    pub mouse_pos: (AtomicF32, AtomicF32),
    /// 鼠标按键状态（左、中、右，含边沿检测）。
    mouse_state: MouseStateDirtyMap,
    /// 光标样式（互斥锁）。
    pub cursor_style: Mutex<Option<CursorStyle>>,
    /// 鼠标位置（缩放后，与物理像素对应）。
    pub scaled_mouse_pos: (AtomicF32, AtomicF32),
    // TODO: 把scroll_wheel专门弄一个类型，用于累积滚轮行程，须有与KeyState和MouseState类似的累积取走结构
    /// 滚轮偏移量。
    pub scroll_wheel: (AtomicF32, AtomicF32),
    /// 键盘状态（含边沿检测）。
    key_state: KeyStateDirtyMap,
    /// 窗口是否处于激活状态（由显示线程更新）。
    pub is_active: AtomicBool,
    /// 运行标志：显示线程检查该标志以决定是否继续循环。
    /// 主线程可将其置为 `false` 以请求退出。
    pub is_running: AtomicBool,
    /// 更新钩子：显示线程每帧开始执行一次的用户闭包。
    ///
    /// 由主线程通过 [`set_update_hook`](Self::set_update_hook) /
    /// [`clear_update_hook`](Self::clear_update_hook) 整体替换或清除。
    /// 渲染线程会缓存最近一次成功获取的句柄，以便在 `try_lock` 失败时复用。
    pub(crate) update_hook: Mutex<Option<SharedUpdateHook>>,
}

impl ExchangeLayer {
    const RETRY_TIMES: usize = 2;
    const RETRY_DELAY: Duration = Duration::from_millis(100);

    /// 创建一个新的 `ExchangeLayer`，所有字段为默认值。
    pub fn new() -> ExchangeLayer {
        ExchangeLayer {
            update_signal: Default::default(),
            apply_signal: Default::default(),

            window_size: (Default::default(), Default::default()),
            window_position: (Default::default(), Default::default()),
            title: Mutex::new(None),
            topmost: Default::default(),
            background_color: Default::default(),
            cursor_visibility: AtomicBool::new(true),
            target_fps: AtomicUsize::new(60),                           // 0 为无限制
            fps: Default::default(),
            mouse_pos: (Default::default(), Default::default()),
            mouse_state: Default::default(),
            cursor_style: Mutex::new(None),
            scaled_mouse_pos: (Default::default(), Default::default()),
            scroll_wheel: (Default::default(), Default::default()),
            key_state: Default::default(),
            is_active: Default::default(),
            is_running: Default::default(),
            update_hook: Mutex::new(None),
        }
    }

    pub fn get_key_state(&self) -> KeyState<'_> {
        KeyState::new(&self.key_state)
    }
    pub fn get_mouse_state(&self) -> MouseState<'_> {
        MouseState::new(&self.mouse_state)
    }

    /// 设置显示线程每帧开始执行的更新钩子。
    ///
    /// 与早期把钩子作为 `ArrayViewer::run` 参数传递的方式不同，钩子存放在
    /// 交换层中，可以在显示线程启动前**预先注册**，也可以在运行期间随时
    /// **替换**，无需重启窗口。
    ///
    /// # 参数
    /// - `hook`: 一段 `Fn(Arc<ExchangeLayer>)` 闭包，收到显示线程当前的
    ///   交换层引用。通常用于按需刷新/同步数据。
    ///
    /// # 线程安全
    /// 闭包只需满足 `Send`（不必 `Sync`），调用会被内部互斥锁串行化。
    pub fn set_update_hook(&self, hook: impl Fn(Arc<ExchangeLayer>) + Send + 'static) {
        let mut guard = self
            .update_hook
            .lock()
            .expect("[ExchangeLayer::set_update_hook] update_hook mutex poisoned");
        *guard = Some(SharedUpdateHook::new(hook));
    }

    /// 清除当前更新钩子，渲染线程在后续帧将不再执行任何钩子。
    pub fn clear_update_hook(&self) {
        let mut guard = self
            .update_hook
            .lock()
            .expect("[ExchangeLayer::clear_update_hook] update_hook mutex poisoned");
        *guard = None;
    }


    // ---------- 公共入口 ----------
    /// 从窗口读取当前状态，更新交换层的输入相关字段。
    ///
    /// 该方法由显示线程在每帧调用，根据 `update_signal` 决定读取哪些字段。
    /// 读取后会自动清除 `update_signal`。
    ///
    /// # 参数
    /// - `window`：可变引用到 minifb 窗口。
    pub(crate) fn update_from_window(&self, window: &mut Window) {
        let update_signal: UpdateSignalNonAtomic = self.update_signal.bits.load(Ordering::Relaxed).into();
        if update_signal.get(UpdateFlag::WindowSize)        { self.update_window_size(window);      }
        if update_signal.get(UpdateFlag::WindowPosition)    { self.update_window_position(window);  }
        if update_signal.get(UpdateFlag::MousePos)          { self.update_mouse_pos(window);        }
        if update_signal.get(UpdateFlag::ScaledMousePos)    { self.update_scaled_mouse_pos(window); }
        if update_signal.get(UpdateFlag::MouseState)        { self.update_mouse_state(window);      }
        if update_signal.get(UpdateFlag::ScrollWheel)       { self.update_scroll_wheel(window);     }
        if update_signal.get(UpdateFlag::KeyState)          { self.update_key_state(window);        }
        if update_signal.get(UpdateFlag::IsActive)          { self.update_is_active(window);        }
    }

    /// 将交换层中的配置数据应用到窗口。
    ///
    /// 该方法由显示线程在每帧调用，根据 `apply_signal` 决定应用哪些字段。
    /// 应用后会自动清除 `apply_signal`。
    ///
    /// # 参数
    /// - `window`：可变引用到 minifb 窗口。
    pub(crate) fn apply_to_window(&self, window: &mut Window) {
        let apply_signal: ApplySignalNonAtomic = self.apply_signal.bits.load(Ordering::Relaxed).into();
        if apply_signal.get(ApplyFlag::WindowPosition)      { self.apply_window_position(window);   }
        // if apply_signal.get(ApplyFlag::Title)               { self.apply_title(window);             }
        self.apply_title(window);
        if apply_signal.get(ApplyFlag::Topmost)             { self.apply_topmost(window);           }
        if apply_signal.get(ApplyFlag::BackgroundColor)     { self.apply_background_color(window);  }
        if apply_signal.get(ApplyFlag::CursorVisibility)    { self.apply_cursor_visibility(window); }
        if apply_signal.get(ApplyFlag::TargetFps)           { self.apply_target_fps(window);        }
        if apply_signal.get(ApplyFlag::CursorStyle)         { self.apply_cursor_style(window);      }
        self.apply_signal.reset();
    }

    // ---------- 更新方法（从窗口读取）----------
    pub(crate) fn update_window_size(&self, window: &Window) {
        let (w, h) = window.get_size();
        self.window_size.0.store(w, Ordering::Relaxed);
        self.window_size.1.store(h, Ordering::Relaxed);
    }

    pub(crate) fn update_window_position(&self, window: &Window) {
        let (x, y) = window.get_position();
        self.window_position.0.store(x, Ordering::Relaxed);
        self.window_position.1.store(y, Ordering::Relaxed);
    }

    pub(crate) fn update_mouse_pos(&self, window: &Window) {
        if let Some((x, y)) = window.get_unscaled_mouse_pos(MouseMode::Discard) {
            self.mouse_pos.0.set(x);
            self.mouse_pos.1.set(y);
        }
    }

    pub(crate) fn update_scaled_mouse_pos(&self, window: &Window) {
        if let Some((x, y)) = window.get_mouse_pos(MouseMode::Discard) {
            self.scaled_mouse_pos.0.set(x);
            self.scaled_mouse_pos.1.set(y);
        }
    }

    pub(crate) fn update_mouse_state(&self, window: &Window) {
        let mouse_state: u8 = u8::from(window.get_mouse_down(MouseButton::Left))
            | u8::from(window.get_mouse_down(MouseButton::Middle)) << 1
            | u8::from(window.get_mouse_down(MouseButton::Right)) << 2;
        // 注意：mouse_state.merge() 是原子操作，将当前帧的按键状态合并到内部状态中。
        // 在每帧显示线程调用 update_from_window 之前，MouseState.update() 已被调用（由主线程或逻辑线程），
        // 以将上一帧的按键状态移动到 prev，并清空当前帧。因此这里 merge 的是本帧的新按键。
        self.mouse_state.merge(mouse_state);
    }

    pub(crate) fn update_scroll_wheel(&self, window: &Window) {
        if let Some((x, y)) = window.get_scroll_wheel() {
            self.scroll_wheel.0.set(x);
            self.scroll_wheel.1.set(y);
        }
    }

    pub(crate) fn update_key_state(&self, window: &Window) {
        let keys = window.get_keys();
        // 同样，KeyState.merge() 合并本帧按下的按键。
        self.key_state.merge(&keys);
    }

    pub(crate) fn update_is_active(&self, window: &mut Window) {
        self.is_active.store(window.is_active(), Ordering::Relaxed);
    }

    // ---------- 应用方法（应用到窗口）----------
    pub(crate) fn apply_window_position(&self, window: &mut Window) {
        let x = self.window_position.0.load(Ordering::Relaxed);
        let y = self.window_position.1.load(Ordering::Relaxed);
        window.set_position(x, y);
    }

    pub(crate) fn apply_title(&self, window: &mut Window) {
        self.apply_option_field(&self.title, |opt| {
            if let Some(title) = opt {
                window.set_title(title);
            } else { window.set_title(
                &format!("fps: {}", self.fps.load(Ordering::Relaxed))
            ); }
        });
    }

    pub(crate) fn apply_topmost(&self, window: &mut Window) {
        window.topmost(self.topmost.load(Ordering::Relaxed));
    }

    pub(crate) fn apply_background_color(&self, window: &mut Window) {
        let color = self.background_color.load(Ordering::Relaxed);
        let r = ((color >> 16) & 0xFF) as u8;
        let g = ((color >> 8) & 0xFF) as u8;
        let b = (color & 0xFF) as u8;
        window.set_background_color(r, g, b);
    }

    pub(crate) fn apply_cursor_visibility(&self, window: &mut Window) {
        window.set_cursor_visibility(self.cursor_visibility.load(Ordering::Relaxed));
    }

    pub(crate) fn apply_target_fps(&self, window: &mut Window) {
        window.set_target_fps(self.target_fps.load(Ordering::Relaxed));
    }

    pub(crate) fn apply_cursor_style(&self, window: &mut Window) {
        self.apply_option_field(&self.cursor_style, |opt| {
            if let Some(style) = opt {
                window.set_cursor_style(*style);
            }
        });
    }

    // ---------- 辅助：带重试的锁操作 ----------
    /// 尝试获取互斥锁，最多重试3次，每次间隔50ms。
    /// 成功获取后调用 `apply` 闭包，传入 `Option<&T>`。
    /// 若三次均失败则静默忽略并输出警告。
    fn apply_option_field<T, F>(&self, mutex: &Mutex<Option<T>>, mut apply: F)
    where
        F: FnMut(Option<&T>),
    {
        for attempt in 0..=Self::RETRY_TIMES {
            if let Ok(guard) = mutex.try_lock() {
                apply(guard.as_ref());
                break;
            } else if attempt == Self::RETRY_TIMES {
                eprintln!(
                    "[warning] The mutex lock of type:{} object in thread:{} is over delay",
                    type_name::<T>(),
                    thread::current().name().unwrap_or(
                        &*format!("id:{:?}", thread::current().id())
                    )
                );
                break;
            }
            thread::sleep(Self::RETRY_DELAY);
        }
    }
}