use std::fmt;
use minifb::Window;

/// 更新钩子：接收窗口可变引用的用户闭包句柄。
///
/// 入参传 `&mut Window` 而非 `Arc<ExchangeLayer>`：主线程在创建闭包前本就可以
/// 通过 `get_exchange_layer()` 捕获 `Arc<ExchangeLayer>`；把窗口本身交给钩子，
/// 使钩子能直接执行窗口级操作（读取键鼠输入、修改标题等），职责更明确。
///
/// 该类型是纯“拥有型”句柄（内部仅一层 `Box`，不再包 `Arc`）：渲染线程通过
/// “邮箱 + 代数”模型领取后即独占该钩子，因此无需克隆。
pub struct UpdateHook {
    /// 真正的用户闭包。
    f: Box<dyn Fn(&mut Window) + Send>,
}

impl UpdateHook {
    /// 调用钩子，传入当前窗口的可变引用。
    pub(crate) fn call(&self, window: &mut Window) {
        (self.f)(window);
    }
}

impl fmt::Debug for UpdateHook {
    /// 闭包不可打印，仅输出不透明句柄信息。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateHook").finish_non_exhaustive()
    }
}

/// 包装函数：把一个 `Fn(&mut Window)` 闭包装箱成 [`UpdateHook`]。
pub fn box_hook(hook: impl Fn(&mut Window) + Send + 'static) -> UpdateHook {
    UpdateHook { f: Box::new(hook) }
}
