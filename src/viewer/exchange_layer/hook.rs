//! 定义“更新钩子”：一段由主线程设置、显示线程每帧执行一次的用户逻辑闭包。
//!
//! # 背景
//! 早期设计中更新钩子（`update_hook`）作为 `ArrayViewer::run` 的参数传入，
//! 一旦线程启动便无法替换。为了让钩子可以在窗口运行期间被主线程随时
//! **设置 / 替换 / 清除**，本模块把钩子封装成可廉价克隆的共享句柄
//! [`SharedUpdateHook`]，并存储在 `ExchangeLayer` 中由渲染线程逐帧取出调用。
//!
//! # 线程安全
//! 用户提供的闭包只需满足 `Send`（不必 `Sync`）：句柄内部用一个 `Mutex`
//! 串行化“调用”动作。由于 `Mutex<T: Send>` 自动满足 `Send + Sync`，
//! 即使闭包捕获了仅 `Send` 的内容，也能安全地跨线程共享该句柄，
//! 无需任何 `unsafe` 代码。

use crate::viewer::exchange_layer::ExchangeLayer;
use std::fmt;
use std::sync::{Arc, Mutex};

/// 共享的更新钩子句柄。
///
/// # 语义
/// - 主线程通过 [`ExchangeLayer::set_update_hook`] 以**整体替换**的方式换入新钩子，
///   从不就地修改闭包内容。
/// - 渲染线程维护一份最近一次成功获取的句柄副本（缓存）；当从 `ExchangeLayer`
///   读取钩子时发生锁竞争（`try_lock` 失败），仍可复用该副本，避免整帧跳过更新。
/// - 句柄可廉价克隆（内部为 `Arc`），克隆后与原句柄共享同一闭包。
#[derive(Clone)]
pub(crate) struct SharedUpdateHook {
    /// 闭包的共享存储。
    inner: Arc<UpdateHookInner>,
}

/// 钩子的内部存储。
///
/// 闭包放在 `Mutex` 中：调用时短暂持锁、串行执行。这一层互斥使仅 `Send`
/// 的闭包可以被放进 `Arc` 中跨线程共享（`Mutex<T: Send>` 即 `Send + Sync`）。
struct UpdateHookInner {
    /// 真正的用户闭包。
    f: Mutex<Box<dyn Fn(Arc<ExchangeLayer>) + Send>>,
}

impl SharedUpdateHook {
    /// 使用一个新的闭包创建共享钩子句柄。
    pub(crate) fn new(f: impl Fn(Arc<ExchangeLayer>) + Send + 'static) -> Self {
        Self {
            inner: Arc::new(UpdateHookInner {
                f: Mutex::new(Box::new(f)),
            }),
        }
    }

    /// 调用钩子，传入显示线程当前的 `ExchangeLayer` 共享引用。
    ///
    /// 调用是线程安全的（内部加锁串行执行），但按本库的线程模型约定，
    /// 钩子通常只由渲染线程调用。
    pub(crate) fn call(&self, layer: Arc<ExchangeLayer>) {
        let guard = self
            .inner
            .f
            .lock()
            .expect("[SharedUpdateHook::call] hook mutex poisoned");
        (guard)(layer);
    }
}

impl fmt::Debug for SharedUpdateHook {
    /// 闭包不可打印，仅输出不透明句柄信息。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SharedUpdateHook").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 构造一个计数钩子：每次调用递增 `counter`。
    fn counting_hook(counter: Arc<AtomicU32>) -> impl Fn(Arc<ExchangeLayer>) + Send {
        move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn hook_calls_increment_counter() {
        let counter = Arc::new(AtomicU32::new(0));
        let hook = SharedUpdateHook::new(counting_hook(Arc::clone(&counter)));
        let layer: Arc<ExchangeLayer> = Arc::from(ExchangeLayer::new());

        hook.call(Arc::clone(&layer));
        hook.call(Arc::clone(&layer));
        assert_eq!(counter.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn hook_clone_shares_closure() {
        let counter = Arc::new(AtomicU32::new(0));
        let hook = SharedUpdateHook::new(counting_hook(Arc::clone(&counter)));
        let clone = hook.clone();
        let layer: Arc<ExchangeLayer> = Arc::from(ExchangeLayer::new());

        // 两个句柄指向同一个闭包：调用任意一个都会递增同一计数器。
        hook.call(Arc::clone(&layer));
        clone.call(Arc::clone(&layer));
        clone.call(Arc::clone(&layer));
        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn hook_thread_safe_concurrent_calls() {
        let counter = Arc::new(AtomicU32::new(0));
        let hook = Arc::new(SharedUpdateHook::new(counting_hook(Arc::clone(&counter))));
        let layer: Arc<ExchangeLayer> = Arc::from(ExchangeLayer::new());

        // 多个线程并发调用同一句柄：内部互斥锁保证不会丢失更新。
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let hook = Arc::clone(&hook);
                let layer = Arc::clone(&layer);
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        hook.call(Arc::clone(&layer));
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(counter.load(Ordering::Relaxed), 400);
    }
}
