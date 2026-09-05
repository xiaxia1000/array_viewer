//! 提供原子浮点数（f32）的包装类型。

use std::sync::atomic::{AtomicU32, Ordering};
use std::fmt;

/// 原子 f32，内部使用 `AtomicU32` 存储 IEEE-754 位表示。
///
/// 该类型提供了加载、存储和简便的 get/set 方法，并实现了 `Send` + `Sync`。
#[repr(C, align(4))]
pub struct AtomicF32 {
    v: AtomicU32,
}

impl AtomicF32 {
    /// 使用给定的初始值创建一个新的 `AtomicF32`。
    pub const fn new(value: f32) -> Self {
        Self {
            v: AtomicU32::new(value.to_bits()),
        }
    }

    /// 以指定的内存顺序加载值。
    #[inline]
    pub fn load(&self, order: Ordering) -> f32 {
        f32::from_bits(self.v.load(order))
    }

    /// 以指定的内存顺序存储值。
    #[inline]
    pub fn store(&self, value: f32, order: Ordering) {
        self.v.store(value.to_bits(), order)
    }

    /// 使用 Relaxed 顺序获取当前值。
    #[inline]
    pub fn get(&self) -> f32 {
        self.load(Ordering::Relaxed)
    }

    /// 使用 Relaxed 顺序设置新值。
    #[inline]
    pub fn set(&self, value: f32) {
        self.store(value, Ordering::Relaxed)
    }
}

/* ---------- 基础 Trait 实现 ---------- */

impl Default for AtomicF32 {
    fn default() -> Self {
        Self::new(0.0)
    }
}

impl From<f32> for AtomicF32 {
    fn from(value: f32) -> Self {
        Self::new(value)
    }
}

impl fmt::Debug for AtomicF32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AtomicF32")
            .field(&self.load(Ordering::Relaxed))
            .finish()
    }
}

// f32 已经是 Send + Sync，AtomicU32 也是 Send + Sync
unsafe impl Send for AtomicF32 {}
unsafe impl Sync for AtomicF32 {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_f32_roundtrip() {
        let v = AtomicF32::new(3.5);
        assert_eq!(v.get(), 3.5);

        v.set(-1.25);
        assert_eq!(v.get(), -1.25);

        v.store(100.0, Ordering::Relaxed);
        assert_eq!(v.load(Ordering::Relaxed), 100.0);

        assert_eq!(AtomicF32::from(2.0).get(), 2.0);
        assert_eq!(AtomicF32::default().get(), 0.0);
    }
}