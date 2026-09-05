//! 鼠标按钮状态跟踪，支持边沿检测（按下/释放）和线程安全。

use std::sync::atomic::{AtomicU8, Ordering};

/// 鼠标按钮位掩码常量。
pub(crate) const LEFT_BUTTON: u8 = 1 << 0;
pub(crate) const MIDDLE_BUTTON: u8 = 1 << 1;
pub(crate) const RIGHT_BUTTON: u8 = 1 << 2;
/// 所有有效按钮的掩码（低 3 位）。
pub(crate) const BUTTON_MASK: u8 = 0b0000_0111;

/// 线程安全的鼠标按键脏位累积器。
///
/// 该结构负责接收来自任意线程的按键合并操作，并在主线程请求时一次性取出并清空。
/// 它的设计允许输入事件（如窗口回调、游戏手柄轮询等）在多个线程中并发调用 [`merge`](Self::merge)，
/// 而不会发生数据竞争。取出操作 [`take`](Self::take) 是原子的，确保主线程能安全地获得自上次取出以来的所有按键。
///
/// # 内部实现
/// 使用一个 `AtomicU8` 存储累积的按钮位，每位对应一个按钮（左、中、右）。
/// 合并操作使用原子按位或，取出使用原子交换。
#[derive(Debug)]
pub(crate) struct MouseStateDirtyMap {
    /// 原子位掩码，低 3 位分别表示左、中、右按钮是否被按下。
    bits: AtomicU8,
}

impl MouseStateDirtyMap {
    /// 创建一个空的累积器，所有位初始为零。
    pub(crate) const fn new() -> Self {
        Self {
            bits: AtomicU8::new(0),
        }
    }

    /// 将给定的掩码（低 3 位）原子地合并到累积器中。
    ///
    /// 该方法可被多个线程并发调用，每个按钮位独立进行原子按位或操作。
    /// 传入的掩码将在下一次 [`take`](Self::take) 时被主线程获取。
    ///
    /// # 参数
    /// - `mask`: 只有低 3 位有效，分别对应左、中、右按钮。
    pub(crate) fn merge(&self, mask: u8) {
        let mask = mask & BUTTON_MASK;
        if mask != 0 {
            self.bits.fetch_or(mask, Ordering::SeqCst);
        }
    }

    /// 原子地读取并清空累积器，返回读取到的位掩码。
    ///
    /// 此方法通常由主线程在每帧开始时调用，用于获取自上一帧以来新按下的按钮。
    /// 调用后内部状态重置为零，后续的合并将从空白开始。
    ///
    /// # 返回值
    /// 一个 `u8`，表示自上次取出以来被合并的所有按钮位（低 3 位有效）。
    pub(crate) fn take(&self) -> u8 {
        self.bits.swap(0, Ordering::SeqCst)
    }

    #[allow(unused)]
    /// 将所有内部位清零（用于重置）。
    fn reset(&self) {
        self.bits.store(0, Ordering::Relaxed);
    }
}

/// 手动实现 `Default`，以便可以方便地创建默认累积器。
impl Default for MouseStateDirtyMap {
    fn default() -> Self {
        Self::new()
    }
}

/// 鼠标状态结构，记录当前帧和上一帧的三个按钮（左、中、右）的状态，并提供边沿检测。
///
/// # 设计说明
/// 该结构持有对 [`MouseStateDirtyMap`] 的引用，并维护两个普通字段：
/// - `now`: 当前帧的按钮状态（由 [`MouseStateDirtyMap::take`] 获得）。
/// - `pre`: 上一帧的按钮状态（由上一帧的 `now` 复制而来）。
///
/// 这两个字段由主线程独占访问，因此不需要原子操作。而实际的按钮累积发生在共享的
/// [`MouseStateDirtyMap`] 中，它可以在任意线程被合并。
///
/// # 生命周期
/// `MouseState` 借用 [`MouseStateDirtyMap`]，因此它的生命周期不能超过后者。
/// 通常，`MouseStateDirtyMap` 被创建一次，并在整个程序运行期间存在，
/// 而 `MouseState` 作为每帧的视图由它构造。
///
/// # 使用约定
/// 1. 在每帧开始时调用 [`update`](Self::update)（需要 `&mut self`）：
///    - 将 `now` 复制到 `pre`。
///    - 从共享的 `dirty_map` 中取出新按钮状态写入 `now`。
///    - 取出后 `dirty_map` 为空，等待本帧期间新的 `merge` 调用。
/// 2. 在帧处理期间（通常在输入事件回调中），通过 [`MouseStateDirtyMap::merge`]
///    将检测到的按钮合并到 `dirty_map`。合并可以在任意线程进行。
/// 3. 查询方法（如 [`is_left_pressed`](Self::is_left_pressed)、
///    [`is_left_down`](Self::is_left_down)、[`is_left_up`](Self::is_left_up)）
///    基于 `now` 和 `pre` 提供状态和边沿检测。
///
/// # 线程安全
/// - `MouseStateDirtyMap` 是 `Sync`，可跨线程共享。
/// - `MouseState` 包含非原子字段，不是 `Sync`，应由主线程独占使用（通常是单线程的 UI 逻辑）。
/// - 通过借用共享的 `dirty_map`，实现了安全的多线程按钮收集与主线程状态更新的分离。
#[derive(Debug)]
pub struct MouseState<'el> {
    /// 引用共享的按钮累积器，用于每帧取出新按钮。
    dirty_map: &'el MouseStateDirtyMap,
    /// 当前帧的按钮位掩码。
    now: u8,
    /// 上一帧的按钮位掩码。
    pre: u8,
}

impl<'el> MouseState<'el> {
    /* ---------- 构造 ---------- */

    /// 从给定的 [`MouseStateDirtyMap`] 创建一个新的 `MouseState`。
    ///
    /// 初始时 `now` 和 `pre` 均为 0。
    ///
    /// # 参数
    /// - `dirty_map`: 共享的按钮累积器引用。
    pub(crate) const fn new(dirty_map: &'el MouseStateDirtyMap) -> Self {
        Self {
            dirty_map,
            now: 0,
            pre: 0,
        }
    }

    /* ---------- 帧更新 ---------- */

    /// 每帧开始时调用，更新当前帧和上一帧的状态。
    ///
    /// 操作顺序：
    /// 1. `pre = now`：将上一帧的 `now` 保存为 `pre`。
    /// 2. `now = dirty_map.take()`：从共享累积器中取出新按钮状态，并清空累积器。
    ///
    /// 此后，`dirty_map` 为空，等待本帧期间新的 `merge` 调用，这些调用将在下一帧生效。
    ///
    /// # 注意
    /// 此方法需要 `&mut self`，因为它修改 `now` 和 `pre`。调用者应确保只有主线程
    /// 拥有 `MouseState` 的可变访问权，其他线程只能通过 `dirty_map` 合并按钮。
    pub fn update(&mut self) {
        self.pre = self.now;
        self.now = self.dirty_map.take();
    }

    /* ---------- 当前状态查询 ---------- */

    /// 左键当前是否按下。
    pub fn is_left_pressed(&self) -> bool {
        self.now & LEFT_BUTTON != 0
    }

    /// 中键当前是否按下。
    pub fn is_middle_pressed(&self) -> bool {
        self.now & MIDDLE_BUTTON != 0
    }

    /// 右键当前是否按下。
    pub fn is_right_pressed(&self) -> bool {
        self.now & RIGHT_BUTTON != 0
    }

    /* ---------- 边沿检测（Down / Up） ---------- */

    /// 检测左键是否刚刚被按下（上升沿）。
    pub fn is_left_down(&self) -> bool {
        (self.now & LEFT_BUTTON != 0) && (self.pre & LEFT_BUTTON == 0)
    }

    /// 检测左键是否刚刚被释放（下降沿）。
    pub fn is_left_up(&self) -> bool {
        (self.now & LEFT_BUTTON == 0) && (self.pre & LEFT_BUTTON != 0)
    }

    /// 检测中键是否刚刚被按下。
    pub fn is_middle_down(&self) -> bool {
        (self.now & MIDDLE_BUTTON != 0) && (self.pre & MIDDLE_BUTTON == 0)
    }

    /// 检测中键是否刚刚被释放。
    pub fn is_middle_up(&self) -> bool {
        (self.now & MIDDLE_BUTTON == 0) && (self.pre & MIDDLE_BUTTON != 0)
    }

    /// 检测右键是否刚刚被按下。
    pub fn is_right_down(&self) -> bool {
        (self.now & RIGHT_BUTTON != 0) && (self.pre & RIGHT_BUTTON == 0)
    }

    /// 检测右键是否刚刚被释放。
    pub fn is_right_up(&self) -> bool {
        (self.now & RIGHT_BUTTON == 0) && (self.pre & RIGHT_BUTTON != 0)
    }

    /* ---------- 快照 ---------- */

    /// 返回 `(now, pre)` 两个字节的快照，用于调试或外部状态保存。
    pub fn snapshot(&self) -> (u8, u8) {
        (self.now, self.pre)
    }

    /* ---------- 重置 ---------- */

    #[expect(dead_code, reason = "maybe use in the future")]
    /// 重置所有状态为 0，包括共享累积器。
    ///
    /// 该方法会同时清空 `dirty_map` 和本地的 `now`、`pre`。
    /// 通常用于场景切换或重新开始。
    fn reset(&mut self) {
        self.dirty_map.reset();
        self.now = 0;
        self.pre = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_map_merge_take_once() {
        let map = MouseStateDirtyMap::new();
        assert_eq!(map.take(), 0);

        // 多次合并（含重复按钮）后一次性取出
        map.merge(LEFT_BUTTON);
        map.merge(LEFT_BUTTON | RIGHT_BUTTON);
        assert_eq!(map.take(), LEFT_BUTTON | RIGHT_BUTTON);

        // take 后清空；merge 时超出低 3 位的位被屏蔽
        assert_eq!(map.take(), 0);
        map.merge(0b1111_1111);
        assert_eq!(map.take(), BUTTON_MASK);
    }

    #[test]
    fn dirty_map_thread_safe() {
        use std::sync::Arc;

        let map = Arc::new(MouseStateDirtyMap::new());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let bit = 1u8 << (i % 3);
                let m = Arc::clone(&map);
                std::thread::spawn(move || m.merge(bit))
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(map.take(), BUTTON_MASK);
    }

    #[test]
    fn edge_detection_sequence() {
        let map = MouseStateDirtyMap::new();
        let mut state = MouseState::new(&map);

        // 初始：无按钮
        state.update();
        assert!(!state.is_left_down());
        assert!(!state.is_left_up());
        assert!(!state.is_left_pressed());

        // 按下 → 上升沿
        map.merge(LEFT_BUTTON);
        state.update();
        assert!(state.is_left_down());
        assert!(state.is_left_pressed());
        assert!(!state.is_left_up());

        // 持续按住 → 不再触发按下沿
        map.merge(LEFT_BUTTON);
        state.update();
        assert!(!state.is_left_down());
        assert!(state.is_left_pressed());

        // 释放 → 下降沿
        state.update();
        assert!(state.is_left_up());
        assert!(!state.is_left_pressed());
        assert!(!state.is_left_down());
    }

    #[test]
    fn multiple_buttons_independent() {
        let map = MouseStateDirtyMap::new();
        let mut state = MouseState::new(&map);
        state.update();

        // 同时按下左、右键
        map.merge(LEFT_BUTTON | RIGHT_BUTTON);
        state.update();
        assert!(state.is_left_down());
        assert!(state.is_right_down());
        assert!(!state.is_middle_down());
        assert!(state.is_left_pressed() && state.is_right_pressed());

        // 释放右键、保留左键
        map.merge(LEFT_BUTTON);
        state.update();
        assert!(state.is_right_up());
        assert!(!state.is_right_pressed());
        assert!(state.is_left_pressed());

        // 按下中键（左键仍按住，需继续合并）
        map.merge(LEFT_BUTTON | MIDDLE_BUTTON);
        state.update();
        assert!(state.is_middle_down());
        assert!(state.is_middle_pressed());
        assert!(!state.is_middle_up());
        assert!(state.is_left_pressed());
        assert!(!state.is_left_down());
        assert!(!state.is_right_pressed());
    }

    #[test]
    fn snapshot_and_reset() {
        let map = MouseStateDirtyMap::new();
        let mut state = MouseState::new(&map);
        state.update();
        map.merge(LEFT_BUTTON | MIDDLE_BUTTON);
        state.update();

        let (now, pre) = state.snapshot();
        assert_eq!(now, LEFT_BUTTON | MIDDLE_BUTTON);
        assert_eq!(pre, 0);

        state.reset();
        assert!(!state.is_left_pressed());
        assert!(!state.is_middle_pressed());
        assert!(!state.is_left_down());
        assert!(!state.is_left_up());
        assert_eq!(state.snapshot(), (0, 0));
    }
}
