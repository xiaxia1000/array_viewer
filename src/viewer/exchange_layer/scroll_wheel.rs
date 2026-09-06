//! 鼠标滚轮行程状态：与 `KeyState` / `MouseState` 同构的“脏累计 → 取走”结构。
//!
//! 滚轮与按键/按钮的关键区别在于它描述的是**增量事件**（本帧滚动了多少），
//! 而不是瞬时电平（当前是否按下）。因此累积单元不能只保存“最近一帧”的值，
//! 否则主线程两次读取之间发生的滚动会被覆盖丢失。
//!
//! 本模块提供两层：
//! - [`ScrollWheelAccumulator`]：线程安全的累积器。显示线程把每帧窗口读到的
//!   增量 [merge](ScrollWheelAccumulator::merge) 进来（原子累加）；
//! - [`ScrollWheel`]：主线程独占的帧视图。按帧 [update](ScrollWheel::update)
//!   从累积器 [take](ScrollWheelAccumulator::take) 走累计行程并清零，再查询。
//!
//! 累加依赖 `atomic_float::AtomicF32::fetch_add`（内部为 CAS 循环），
//! 因此无需加锁即可安全累加。

use atomic_float::AtomicF32;
use std::sync::atomic::Ordering;

/// 线程安全的滚轮行程累积器。
///
/// # 内部实现
/// 使用两个 `AtomicF32` 分别累计水平（x）与垂直（y）行程：
/// - 累加使用 `fetch_add`（原子、CAS 循环），可被多线程并发调用；
/// - 取走使用 `swap(0.0)`，原子地读出总量并清零。
#[derive(Debug)]
pub(crate) struct ScrollWheelAccumulator {
    /// 累计的水平滚动量。
    x: AtomicF32,
    /// 累计的垂直滚动量。
    y: AtomicF32,
}

impl ScrollWheelAccumulator {
    /// 创建一个空的累积器，累计行程为零。
    pub(crate) const fn new() -> Self {
        Self {
            x: AtomicF32::new(0.0),
            y: AtomicF32::new(0.0),
        }
    }

    /// 将本帧的滚轮增量（`x`, `y`）累加进累积器。
    ///
    /// 该方法可被多个线程并发调用（显示线程每帧调用一次）。
    /// 增量为 `(0.0, 0.0)` 时直接返回，避免无意义的原子操作。
    pub(crate) fn merge(&self, x: f32, y: f32) {
        if x == 0.0 && y == 0.0 {
            return;
        }
        // fetch_add 的返回值（累加前的旧值）在此无意义，忽略即可。
        self.x.fetch_add(x, Ordering::Relaxed);
        self.y.fetch_add(y, Ordering::Relaxed);
    }

    /// 原子地取走并清零，返回自上次取走以来累计的总行程 `(x, y)`。
    ///
    /// 该方法通常由主线程在每帧开始时调用。
    pub(crate) fn take(&self) -> (f32, f32) {
        (
            self.x.swap(0.0, Ordering::Relaxed),
            self.y.swap(0.0, Ordering::Relaxed),
        )
    }

    /// 清零所有内部累计值（用于重置）。
    pub(crate) fn reset(&self) {
        self.x.store(0.0, Ordering::Relaxed);
        self.y.store(0.0, Ordering::Relaxed);
    }
}

/// 手动实现 `Default`，便于默认构造空累积器。
impl Default for ScrollWheelAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

/// 滚轮行程的帧视图，由主线程独占访问。
///
/// # 使用约定
/// 1. 每帧开始时调用 [`update`](Self::update)（需要 `&mut self`）：
///    从共享累积器中取走自上次取走以来累计的总行程，并清空累积器。
/// 2. 随后通过 [`x`](Self::x) / [`y`](Self::y) 查询本周期内的滚动量。
///
/// # 线程安全
/// - `ScrollWheelAccumulator` 是 `Sync`，可跨线程共享（显示线程往里累加）。
/// - `ScrollWheel` 持有非原子字段，由主线程独占使用，与 `KeyState` / `MouseState`
///   的线程模型一致。
#[derive(Debug)]
pub struct ScrollWheel<'el> {
    /// 引用共享的行程累积器，用于每帧取走累计行程。
    accumulator: &'el ScrollWheelAccumulator,
    /// 本周期内的水平滚动量（自上次 update 以来）。
    x: f32,
    /// 本周期内的垂直滚动量（自上次 update 以来）。
    y: f32,
}

impl<'el> ScrollWheel<'el> {
    /// 从给定的 [`ScrollWheelAccumulator`] 创建一个新的帧视图。
    ///
    /// 初始时本周期行程为 `(0.0, 0.0)`。
    pub(crate) const fn new(accumulator: &'el ScrollWheelAccumulator) -> Self {
        Self {
            accumulator,
            x: 0.0,
            y: 0.0,
        }
    }

    /// 每帧开始时调用，取走自上次取走以来累计的滚轮行程。
    ///
    /// 调用后累积器被清零，本帧内新的 `merge` 将留到下一次 `update` 再取走。
    pub fn update(&mut self) {
        (self.x, self.y) = self.accumulator.take();
    }

    /// 返回本周期内的水平滚动量。
    pub fn x(&self) -> f32 {
        self.x
    }

    /// 返回本周期内的垂直滚动量。
    pub fn y(&self) -> f32 {
        self.y
    }

    /// 返回 `(x, y)` 快照。
    pub fn snapshot(&self) -> (f32, f32) {
        (self.x, self.y)
    }

    /// 本周期内是否没有任何滚动。
    pub fn is_zero(&self) -> bool {
        self.x == 0.0 && self.y == 0.0
    }

    /// 重置：同时清空共享累积器与本地行程。
    #[expect(dead_code, reason = "maybe use in the future")]
    pub(crate) fn reset(&mut self) {
        self.accumulator.reset();
        self.x = 0.0;
        self.y = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accumulator_merge_take_once() {
        let acc = ScrollWheelAccumulator::new();
        assert_eq!(acc.take(), (0.0, 0.0));

        // 多次累加后一次性取走
        acc.merge(1.0, 2.0);
        acc.merge(3.0, -1.0);
        assert_eq!(acc.take(), (4.0, 1.0));

        // take 后清零；零增量 merge 不会留下痕迹
        assert_eq!(acc.take(), (0.0, 0.0));
        acc.merge(0.0, 0.0);
        assert_eq!(acc.take(), (0.0, 0.0));
    }

    #[test]
    fn accumulator_thread_safe() {
        use std::sync::Arc;

        let acc = Arc::new(ScrollWheelAccumulator::new());
        // 8 个线程并发累加，验证原子累加不丢增量
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let acc = Arc::clone(&acc);
                std::thread::spawn(move || {
                    let base = i as f32;
                    acc.merge(base, -base);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        // x 累计 0+1+...+7 = 28，y 为其相反数
        assert_eq!(acc.take(), (28.0, -28.0));
    }

    #[test]
    fn view_update_cycle() {
        let acc = ScrollWheelAccumulator::new();
        let mut wheel = ScrollWheel::new(&acc);

        // 初始无滚动
        wheel.update();
        assert!(wheel.is_zero());
        assert_eq!(wheel.snapshot(), (0.0, 0.0));

        // 多帧滚动在两次 update 之间被累积，且取走后不留残余
        acc.merge(10.0, 5.0);
        acc.merge(-3.0, 1.0);
        wheel.update();
        assert_eq!(wheel.x(), 7.0);
        assert_eq!(wheel.y(), 6.0);
        assert!(!wheel.is_zero());

        // 下一帧没有新滚动 → 归零
        wheel.update();
        assert!(wheel.is_zero());
        assert_eq!(wheel.snapshot(), (0.0, 0.0));
    }

    #[test]
    fn view_reset_clears_both() {
        let acc = ScrollWheelAccumulator::new();
        let mut wheel = ScrollWheel::new(&acc);

        acc.merge(2.0, 3.0);
        wheel.reset();
        assert!(wheel.is_zero());
        assert_eq!(acc.take(), (0.0, 0.0));
    }
}
