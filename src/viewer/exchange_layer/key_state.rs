//! 键盘状态跟踪，支持边沿检测（按下/释放）和线程安全。

use std::sync::atomic::{AtomicUsize, Ordering};
use minifb::Key;

/// 编译期计算所需的 usize 槽位数，每个位代表一个键。
const USIZE_COUNT: usize = (Key::Count as usize).div_ceil(usize::BITS as usize);

/// 线程安全的按键脏位累积器。
///
/// 该结构负责接收来自任意线程的按键合并操作，并在主线程请求时一次性取出并清空。
/// 它的设计允许输入事件（如窗口回调、游戏手柄轮询等）在多个线程中并发调用 [`merge`](Self::merge)，
/// 而不会发生数据竞争。取出操作 [`take`](Self::take) 是原子的，确保主线程能安全地获得自上次取出以来的所有按键。
///
/// # 内部实现
/// 按键按位存储在一个 `[AtomicUsize; USIZE_COUNT]` 数组中，每个槽位负责一组按键。
/// 每个槽位的更新使用 CAS（compare-and-swap）循环保证并发安全。
#[derive(Debug)]
pub(crate) struct KeyStateDirtyMap {
    /// 每个槽位存储一组按键的位掩码，使用原子操作保证线程安全。
    raw: [AtomicUsize; USIZE_COUNT],
}

impl KeyStateDirtyMap {
    /// 创建一个空的累积器，所有位初始为零。
    pub(crate) const fn new() -> Self {
        const INIT: AtomicUsize = AtomicUsize::new(0);
        Self {
            raw: [INIT; USIZE_COUNT],
        }
    }

    /// 将按键列表原子地合并到累积器中。
    ///
    /// 该方法可被多个线程并发调用，每个槽位独立进行 CAS 循环更新。
    /// 传入的按键将在下一次 [`take`](Self::take) 时被主线程获取。
    ///
    /// # 参数
    /// - `keys`: 要合并的按键切片，可能包含重复按键。
    pub(crate) fn merge(&self, keys: &[Key]) {
        // 先将所有按键转换成每个槽位的掩码，避免在循环中重复计算。
        let mut masks = [0usize; USIZE_COUNT];
        for &key in keys {
            let (slot, bit) = Self::slot_bit(key);
            masks[slot] |= bit;
        }

        // 对每个非零掩码的槽位执行原子按位或操作。
        for (slot, mask) in masks.iter().enumerate() {
            if *mask == 0 {
                continue;
            }
            let mut old = self.raw[slot].load(Ordering::SeqCst);
            loop {
                let new = old | mask;
                match self.raw[slot].compare_exchange(old, new, Ordering::SeqCst, Ordering::SeqCst)
                {
                    Ok(_) => break,
                    Err(current) => old = current,
                }
            }
        }
    }

    /// 原子地读取并清空累积器，返回读取到的位数组。
    ///
    /// 此方法通常由主线程在每帧开始时调用，用于获取自上一帧以来新按下的按键。
    /// 调用后内部状态重置为零，后续的合并将从空白开始。
    ///
    /// # 返回值
    /// 一个 `[usize; USIZE_COUNT]` 数组，表示自上次取出以来被合并的所有按键位。
    pub(crate) fn take(&self) -> [usize; USIZE_COUNT] {
        let mut result = [0usize; USIZE_COUNT];
        for (slot, r) in result.iter_mut().enumerate() {
            // swap(0) 等价于读取旧值并写入 0，是原子的。
            *r = self.raw[slot].swap(0, Ordering::SeqCst);
        }
        result
    }

    /// 将按键映射到存储槽位和位掩码。
    ///
    /// 该函数是内部辅助方法，被 [`merge`](Self::merge) 和 [`KeyState`] 的查询方法使用。
    #[inline]
    fn slot_bit(key: Key) -> (usize, usize) {
        let idx = key as usize;
        let slot = idx / (usize::BITS as usize);
        let bit = 1usize << (idx % (usize::BITS as usize));
        (slot, bit)
    }

    #[allow(unused)]
    /// 将所有内部位清零（用于重置）。
    fn reset(&self) {
        for slot in 0..USIZE_COUNT {
            self.raw[slot].store(0, Ordering::Relaxed);
        }
    }
}

/// 手动实现 `Default`，以便可以方便地创建默认累积器。
impl Default for KeyStateDirtyMap {
    fn default() -> Self {
        Self::new()
    }
}

/// 键盘状态结构，记录当前帧和上一帧的按键按下情况，并提供边沿检测。
///
/// # 设计说明
/// 该结构持有对 [`KeyStateDirtyMap`] 的引用，并维护两个普通数组：
/// - `now`：当前帧的按键状态（由 [`KeyStateDirtyMap::take`] 获得）。
/// - `pre`：上一帧的按键状态（由上一帧的 `now` 复制而来）。
///
/// 这两个数组由主线程独占访问，因此不需要原子操作。而实际的按键累积发生在共享的
/// [`KeyStateDirtyMap`] 中，它可以在任意线程被合并。
///
/// # 生命周期
/// `KeyState` 借用 [`KeyStateDirtyMap`]，因此它的生命周期不能超过后者。
/// 通常，`KeyStateDirtyMap` 被创建一次，并在整个程序运行期间存在，
/// 而 `KeyState` 作为每帧的视图由它构造。
///
/// # 使用约定
/// 1. 在每帧开始时调用 [`update`](Self::update)（需要 `&mut self`）：
///    - 将 `now` 复制到 `pre`。
///    - 从共享的 `dirty_map` 中取出新按键状态写入 `now`。
///    - 取出后 `dirty_map` 为空，等待本帧期间新的 `merge` 调用。
/// 2. 在帧处理期间（通常在输入事件回调中），通过 [`KeyStateDirtyMap::merge`]
///    将检测到的按键合并到 `dirty_map`。合并可以在任意线程进行。
/// 3. 查询方法（如 [`is_down`](Self::is_down)、
///    [`is_up`](Self::is_up)、[`is_pressed`](Self::is_pressed)）
///    基于 `now` 和 `pre` 提供边沿和状态检测。
///
/// # 线程安全
/// - `KeyStateDirtyMap` 是 `Sync`，可跨线程共享。
/// - `KeyState` 包含非原子字段，不是 `Sync`，应由主线程独占使用（通常是单线程的 UI 逻辑）。
/// - 通过借用共享的 `dirty_map`，实现了安全的多线程按键收集与主线程状态更新的分离。
#[derive(Debug)]
pub struct KeyState<'el> {
    /// 引用共享的按键累积器，用于每帧取出新按键。
    dirty_map: &'el KeyStateDirtyMap,
    /// 当前帧的按键位图。
    now: [usize; USIZE_COUNT],
    /// 上一帧的按键位图。
    pre: [usize; USIZE_COUNT],
}

impl<'el> KeyState<'el> {
    /* ---------- 构造 ---------- */

    /// 从给定的 [`KeyStateDirtyMap`] 创建一个新的 `KeyState`。
    ///
    /// 初始时 `now` 和 `pre` 均为空。
    ///
    /// # 参数
    /// - `dirty_map`: 共享的按键累积器引用。
    pub(crate) const fn new(dirty_map: &'el KeyStateDirtyMap) -> Self {
        Self {
            dirty_map,
            now: [0; USIZE_COUNT],
            pre: [0; USIZE_COUNT],
        }
    }

    /* ---------- 帧更新 ---------- */

    /// 每帧开始时调用，更新当前帧和上一帧的状态。
    ///
    /// 操作顺序：
    /// 1. `pre = now`：将上一帧的 `now` 保存为 `pre`。
    /// 2. `now = dirty_map.take()`：从共享累积器中取出新按键状态，并清空累积器。
    ///
    /// 此后，`dirty_map` 为空，等待本帧期间新的 `merge` 调用，这些调用将在下一帧生效。
    ///
    /// # 注意
    /// 此方法需要 `&mut self`，因为它修改 `now` 和 `pre`。调用者应确保只有主线程
    /// 拥有 `KeyState` 的可变访问权，其他线程只能通过 `dirty_map` 合并按键。
    pub fn update(&mut self) {
        // 先保存当前帧到上一帧
        self.pre.copy_from_slice(&self.now);
        // 从累积器取出新按键
        self.now = self.dirty_map.take();
    }

    /* ---------- 状态查询 ---------- */

    /// 检查当前帧中指定键是否被按下。
    #[inline]
    fn now_bit(&self, key: Key) -> bool {
        let (slot, bit) = KeyStateDirtyMap::slot_bit(key);
        self.now[slot] & bit != 0
    }

    /// 检查上一帧中指定键是否被按下。
    #[inline]
    fn pre_bit(&self, key: Key) -> bool {
        let (slot, bit) = KeyStateDirtyMap::slot_bit(key);
        self.pre[slot] & bit != 0
    }

    /// 检测按键是否刚刚被按下（上升沿）。
    ///
    /// 即当前帧按下且上一帧未按下。
    pub fn is_down(&self, key: Key) -> bool {
        self.now_bit(key) && !self.pre_bit(key)
    }

    /// 检测按键是否刚刚被释放（下降沿）。
    ///
    /// 即当前帧未按下且上一帧按下。
    pub fn is_up(&self, key: Key) -> bool {
        !self.now_bit(key) && self.pre_bit(key)
    }

    /// 检测按键当前是否被按住。
    ///
    /// 即当前帧中该键为按下状态。
    pub fn is_pressed(&self, key: Key) -> bool {
        self.now_bit(key)
    }

    /* ---------- 快照 ---------- */

    /// 返回 `(now, pre)` 两个数组的快照，用于调试或外部状态保存。
    ///
    /// 返回的数组是当前内部状态的副本，后续修改不会影响快照。
    pub fn snapshot(&self) -> ([usize; USIZE_COUNT], [usize; USIZE_COUNT]) {
        (self.now, self.pre)
    }

    /* ---------- 重置 ---------- */

    #[expect(dead_code, reason = "maybe use in the future")]
    /// 重置所有状态为 0，包括共享累积器。
    ///
    /// 该方法会同时清空 `dirty_map` 和本地的 `now`、`pre`。
    /// 通常用于场景切换或重新开始。
    pub(crate) fn reset(&mut self) {
        self.dirty_map.reset();
        self.now = [0; USIZE_COUNT];
        self.pre = [0; USIZE_COUNT];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minifb::Key;

    #[test]
    fn dirty_map_merge_take_once() {
        let map = KeyStateDirtyMap::new();
        // 初始为空
        assert_eq!(map.take(), [0usize; USIZE_COUNT]);

        // 多次合并（含重复键）后一次性取出
        map.merge(&[Key::A]);
        map.merge(&[Key::A, Key::Space, Key::Enter]);
        let taken = map.take();
        for key in [Key::A, Key::Space, Key::Enter] {
            let (slot, bit) = KeyStateDirtyMap::slot_bit(key);
            assert_ne!(taken[slot] & bit, 0, "{key:?} 应被置位");
        }

        // take 后清空
        assert_eq!(map.take(), [0usize; USIZE_COUNT]);
    }

    #[test]
    fn dirty_map_reset() {
        let map = KeyStateDirtyMap::new();
        map.merge(&[Key::A, Key::B]);
        map.reset();
        assert_eq!(map.take(), [0usize; USIZE_COUNT]);
    }

    #[test]
    fn dirty_map_thread_safe() {
        use std::sync::Arc;

        let map = Arc::new(KeyStateDirtyMap::new());
        let keys = [Key::A, Key::B, Key::C, Key::D, Key::E, Key::F, Key::G, Key::H];
        let handles: Vec<_> = keys
            .iter()
            .map(|&k| {
                let m = Arc::clone(&map);
                std::thread::spawn(move || m.merge(&[k]))
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let taken = map.take();
        for key in keys {
            let (slot, bit) = KeyStateDirtyMap::slot_bit(key);
            assert_ne!(taken[slot] & bit, 0, "{key:?} 应被置位");
        }
    }

    #[test]
    fn edge_detection_sequence() {
        let map = KeyStateDirtyMap::new();
        let mut state = KeyState::new(&map);

        // 第 1 帧：无按键
        state.update();
        assert!(!state.is_down(Key::A));
        assert!(!state.is_up(Key::A));
        assert!(!state.is_pressed(Key::A));

        // 第 2 帧：按下 → 上升沿
        map.merge(&[Key::A]);
        state.update();
        assert!(state.is_down(Key::A));
        assert!(state.is_pressed(Key::A));
        assert!(!state.is_up(Key::A));

        // 第 3 帧：持续按住 → 不再触发按下沿
        map.merge(&[Key::A]);
        state.update();
        assert!(!state.is_down(Key::A));
        assert!(state.is_pressed(Key::A));
        assert!(!state.is_up(Key::A));

        // 第 4 帧：释放 → 下降沿
        state.update();
        assert!(state.is_up(Key::A));
        assert!(!state.is_pressed(Key::A));
        assert!(!state.is_down(Key::A));
    }

    #[test]
    fn merge_only_takes_effect_after_update() {
        let map = KeyStateDirtyMap::new();
        let mut state = KeyState::new(&map);
        state.update();

        // 未调用 update 前，merge 的内容不生效（下一帧才被取出）
        map.merge(&[Key::B]);
        assert!(!state.is_pressed(Key::B));
        state.update();
        assert!(state.is_pressed(Key::B));
    }

    #[test]
    fn multiple_keys_are_independent() {
        let map = KeyStateDirtyMap::new();
        let mut state = KeyState::new(&map);
        state.update();

        map.merge(&[Key::A, Key::C]);
        state.update();
        assert!(state.is_down(Key::A));
        assert!(state.is_down(Key::C));
        assert!(!state.is_down(Key::B));

        // A 释放、B 按下、C 保持
        map.merge(&[Key::B, Key::C]);
        state.update();
        assert!(state.is_up(Key::A));
        assert!(state.is_down(Key::B));
        assert!(state.is_pressed(Key::C));
        assert!(!state.is_down(Key::C));
    }

    #[test]
    fn snapshot_and_reset() {
        let map = KeyStateDirtyMap::new();
        let mut state = KeyState::new(&map);
        state.update();
        map.merge(&[Key::Space]);
        state.update();

        let (now, pre) = state.snapshot();
        let (slot, bit) = KeyStateDirtyMap::slot_bit(Key::Space);
        assert_ne!(now[slot] & bit, 0);
        assert_eq!(pre[slot] & bit, 0);

        state.reset();
        assert!(!state.is_pressed(Key::Space));
        assert!(!state.is_down(Key::Space));
        assert!(!state.is_up(Key::Space));
        assert_eq!(state.snapshot(), ([0usize; USIZE_COUNT], [0usize; USIZE_COUNT]));
    }
}