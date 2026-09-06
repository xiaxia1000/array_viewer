//! 定义用于控制更新和应用的信号标志。

use std::ops::{BitOr, Deref};
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicU8, AtomicU16};

// ============================================================
// UpdateSignal
// ============================================================

/// 更新标志枚举，每个变体对应一个位，用于指示需要从窗口读取的状态。
#[derive(Copy, Clone, Debug)]
#[repr(u8)]
pub enum UpdateFlag {
    WindowSize = 1 << 0,
    WindowPosition = 1 << 1,
    MousePos = 1 << 2,
    MouseState = 1 << 3,
    ScaledMousePos = 1 << 4,
    ScrollWheel = 1 << 5,
    KeyState = 1 << 6,
    IsActive = 1 << 7,
}

/// 更新标志的组合类型，使用 `u8` 位图，便于批量操作。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct UpdateFlags(pub u8);

impl UpdateFlags {
    // 预定义常量，每个标志占一位
    pub const WINDOW_SIZE: Self = Self(1 << 0);
    pub const WINDOW_POSITION: Self = Self(1 << 1);
    pub const MOUSE_POS: Self = Self(1 << 2);
    pub const MOUSE_STATE: Self = Self(1 << 3);
    pub const SCALED_MOUSE_POS: Self = Self(1 << 4);
    pub const SCROLL_WHEEL: Self = Self(1 << 5);
    pub const KEY_STATE: Self = Self(1 << 6);
    pub const IS_ACTIVE: Self = Self(1 << 7);
}

// UpdateFlag 与 UpdateFlag 组合成 UpdateFlags
impl BitOr for UpdateFlag {
    type Output = UpdateFlags;
    fn bitor(self, rhs: Self) -> UpdateFlags {
        UpdateFlags::from(self) | UpdateFlags::from(rhs)
    }
}

// UpdateFlag 与 UpdateFlags 组合成 UpdateFlags
impl BitOr<UpdateFlags> for UpdateFlag {
    type Output = UpdateFlags;
    fn bitor(self, rhs: UpdateFlags) -> UpdateFlags {
        UpdateFlags::from(self) | rhs
    }
}

// UpdateFlags 与 UpdateFlag 组合成 UpdateFlags
impl BitOr<UpdateFlag> for UpdateFlags {
    type Output = Self;
    fn bitor(self, rhs: UpdateFlag) -> Self {
        self | UpdateFlags::from(rhs)
    }
}

// 支持按位或组合
impl BitOr for UpdateFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

// 支持单标志转换为组合（方便直接传单个）
impl From<UpdateFlag> for UpdateFlags {
    fn from(f: UpdateFlag) -> Self {
        Self(f as u8)
    }
}

impl Deref for UpdateFlags {
    type Target = u8;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// 原子更新信号，用于指示显示线程需要从窗口读取哪些字段。
///
/// 该信号由主线程设置，由显示线程在每帧开始时读取并自动清除。
/// 使用原子操作，无需加锁。
///
/// 注意：`new()` 初始化为全 1（`u8::MAX`），即默认每帧刷新所有字段；
/// 若只想按需更新，请先调用 [`reset`](Self::reset) 清零。
#[derive(Debug)]
pub struct UpdateSignal {
    pub(crate) bits: AtomicU8,
}

impl UpdateSignal {
    /// 创建一个新的信号，所有位默认为 1（更新全部字段）。
    pub fn new() -> Self {
        Self {
            bits: AtomicU8::new(u8::MAX),
        }
    }

    /// 批量设置指定标志位（置 1）。
    pub fn set(&self, flags: impl Into<UpdateFlags>) {
        self.bits.fetch_or(flags.into().0, Ordering::Relaxed);
    }

    /// 批量清除指定标志位（置 0）。
    pub fn clear(&self, flags: impl Into<UpdateFlags>) {
        let mask = flags.into().0;
        self.bits.fetch_and(!mask, Ordering::Relaxed);
    }

    /// 重置所有标志位为 0。
    pub fn reset(&self) {
        self.bits.store(0, Ordering::SeqCst)
    }

    /// 批量翻转指定标志位。
    pub fn toggle(&self, flags: impl Into<UpdateFlags>) {
        self.bits.fetch_xor(flags.into().0, Ordering::Relaxed);
    }

    /// 读取单个标志位。
    pub fn get(&self, flag: UpdateFlag) -> bool {
        let mask = UpdateFlags::from(flag).0;
        (self.bits.load(Ordering::Relaxed) & mask) != 0
    }
}

impl Default for UpdateSignal {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// ApplySignal
// ============================================================

/// 应用标志枚举，指示需要应用到窗口的配置项。
///
/// 每个变体对应一个位，与 [`ApplyFlags`] 的常量保持一致。
#[derive(Copy, Clone, Debug)]
#[repr(u16)]
pub enum ApplyFlag {
    WindowPosition = 1 << 0,
    Title = 1 << 1,
    Icon = 1 << 2,
    Topmost = 1 << 3,
    BackgroundColor = 1 << 4,
    CursorVisibility = 1 << 5,
    TargetFps = 1 << 6,
    CursorStyle = 1 << 7,
    IsRunning = 1 << 8,
}

/// 应用标志的组合类型，使用 `u16` 位图。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ApplyFlags(pub u16);

impl ApplyFlags {
    pub const WINDOW_POSITION: Self = Self(1 << 0);
    pub const TITLE: Self = Self(1 << 1);
    pub const ICON: Self = Self(1 << 2);
    pub const TOPMOST: Self = Self(1 << 3);
    pub const BACKGROUND_COLOR: Self = Self(1 << 4);
    pub const CURSOR_VISIBILITY: Self = Self(1 << 5);
    pub const TARGET_FPS: Self = Self(1 << 6);
    pub const CURSOR_STYLE: Self = Self(1 << 7);
    pub const IS_RUNNING: Self = Self(1 << 8);
}

// ApplyFlag 与 ApplyFlag 组合成 ApplyFlags
impl BitOr for ApplyFlag {
    type Output = ApplyFlags;
    fn bitor(self, rhs: Self) -> ApplyFlags {
        ApplyFlags::from(self) | ApplyFlags::from(rhs)
    }
}

// ApplyFlag 与 ApplyFlags 组合成 ApplyFlags
impl BitOr<ApplyFlags> for ApplyFlag {
    type Output = ApplyFlags;
    fn bitor(self, rhs: ApplyFlags) -> ApplyFlags {
        ApplyFlags::from(self) | rhs
    }
}

// ApplyFlags 与 ApplyFlag 组合成 ApplyFlags
impl BitOr<ApplyFlag> for ApplyFlags {
    type Output = Self;
    fn bitor(self, rhs: ApplyFlag) -> Self {
        self | ApplyFlags::from(rhs)
    }
}

impl BitOr for ApplyFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl From<ApplyFlag> for ApplyFlags {
    fn from(f: ApplyFlag) -> Self {
        Self(f as u16)
    }
}

impl Deref for ApplyFlags {
    type Target = u16;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// 原子应用信号，用于指示显示线程需要应用哪些窗口配置。
///
/// 由主线程设置，由显示线程在每帧读取并自动清除。
#[derive(Debug)]
pub struct ApplySignal {
    pub(crate) bits: AtomicU16,
}

impl ApplySignal {
    /// 创建一个新的信号，默认所有位为 0。
    pub fn new() -> Self {
        Self {
            bits: AtomicU16::new(0),
        }
    }

    /// 批量设置指定标志位。
    pub fn set(&self, flags: impl Into<ApplyFlags>) {
        self.bits.fetch_or(flags.into().0, Ordering::Relaxed);
    }

    /// 批量清除指定标志位。
    pub fn clear(&self, flags: impl Into<ApplyFlags>) {
        let mask = flags.into().0;
        self.bits.fetch_and(!mask, Ordering::Relaxed);
    }

    /// 重置所有标志位为 0。
    pub fn reset(&self) {
        self.bits.store(0, Ordering::SeqCst)
    }

    /// 批量翻转指定标志位。
    pub fn toggle(&self, flags: impl Into<ApplyFlags>) {
        self.bits.fetch_xor(flags.into().0, Ordering::Relaxed);
    }

    /// 读取单个标志位。
    pub fn get(&self, flag: ApplyFlag) -> bool {
        let mask = ApplyFlags::from(flag).0;
        (self.bits.load(Ordering::Relaxed) & mask) != 0
    }
}

impl Default for ApplySignal {
    fn default() -> Self {
        Self::new()
    }
}

// =========================================================================
// 以下两个非原子类型仅用于内部快速读取信号快照。

#[derive(Debug, Default)]
#[repr(transparent)]
pub(crate) struct UpdateSignalNonAtomic {
    bits: u8,
}

impl From<u8> for UpdateSignalNonAtomic {
    fn from(value: u8) -> Self {
        Self { bits: value }
    }
}

impl UpdateSignalNonAtomic {
    /// 读取单个标志。
    pub(crate) fn get(&self, flag: UpdateFlag) -> bool {
        let mask = UpdateFlags::from(flag).0;
        (self.bits & mask) != 0
    }
}

#[derive(Debug, Default)]
#[repr(transparent)]
pub(crate) struct ApplySignalNonAtomic {
    bits: u16,
}

impl ApplySignalNonAtomic {
    pub(crate) fn get(&self, flag: ApplyFlag) -> bool {
        let mask = ApplyFlags::from(flag).0;
        (self.bits & mask) != 0
    }
}

impl From<u16> for ApplySignalNonAtomic {
    fn from(value: u16) -> Self {
        Self { bits: value }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_signal_new_starts_all_set() {
        // new() 初始化为全 1，保证首帧刷新所有字段
        let s = UpdateSignal::new();
        assert_eq!(s.bits.load(Ordering::Relaxed), u8::MAX);
        assert!(s.get(UpdateFlag::WindowSize));
        assert!(s.get(UpdateFlag::KeyState));
        assert!(s.get(UpdateFlag::IsActive));
    }

    #[test]
    fn update_signal_ops() {
        let s = UpdateSignal::new();
        s.reset();
        assert!(!s.get(UpdateFlag::KeyState));

        s.set(UpdateFlag::KeyState);
        assert!(s.get(UpdateFlag::KeyState));
        assert!(!s.get(UpdateFlag::MousePos));

        s.set(UpdateFlag::KeyState | UpdateFlag::MousePos);
        assert!(s.get(UpdateFlag::KeyState));
        assert!(s.get(UpdateFlag::MousePos));

        s.clear(UpdateFlag::KeyState);
        assert!(!s.get(UpdateFlag::KeyState));
        assert!(s.get(UpdateFlag::MousePos));

        s.toggle(UpdateFlag::MousePos);
        assert!(!s.get(UpdateFlag::MousePos));

        s.reset();
        assert_eq!(s.bits.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn apply_signal_ops() {
        let s = ApplySignal::new();
        assert!(!s.get(ApplyFlag::Title));

        s.set(ApplyFlag::Title | ApplyFlag::IsRunning);
        assert!(s.get(ApplyFlag::Title));
        assert!(s.get(ApplyFlag::IsRunning));
        assert!(!s.get(ApplyFlag::Icon));

        s.clear(ApplyFlag::Title);
        assert!(!s.get(ApplyFlag::Title));
        assert!(s.get(ApplyFlag::IsRunning));

        s.toggle(ApplyFlag::IsRunning);
        assert!(!s.get(ApplyFlag::IsRunning));

        s.reset();
        assert_eq!(s.bits.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn flags_combine() {
        // 枚举与枚举
        let f = UpdateFlag::KeyState | UpdateFlag::MouseState;
        assert_eq!(f.0, (1 << 6) | (1 << 3));
        // 枚举与组合类型
        let f = UpdateFlag::KeyState | UpdateFlags::MOUSE_STATE;
        assert_eq!(f.0, (1 << 6) | (1 << 3));
        let f = UpdateFlags::MOUSE_STATE | UpdateFlag::KeyState;
        assert_eq!(f.0, (1 << 3) | (1 << 6));

        let a = ApplyFlag::Title | ApplyFlag::IsRunning;
        assert_eq!(a.0, (1 << 1) | (1 << 8));
    }

    #[test]
    fn non_atomic_snapshots() {
        let u: UpdateSignalNonAtomic = 0b1000_1000.into();
        assert!(!u.get(UpdateFlag::KeyState));
        assert!(u.get(UpdateFlag::MouseState));
        assert!(!u.get(UpdateFlag::WindowSize));

        let a: ApplySignalNonAtomic = (1 << 1).into();
        assert!(a.get(ApplyFlag::Title));
        assert!(!a.get(ApplyFlag::Icon));
    }
}
