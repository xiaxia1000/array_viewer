use std::ptr;

/// 一个持有类型化可变裸指针（`*mut T`）的包装结构，用于快捷地跨线程传递原始指针。
///
/// # 设计意图
/// 该类型允许将任意类型的可变裸指针包装起来，并在类型层面保留指针所指向的数据类型 `T`。
/// 它本身不管理内存、不保证有效性、不提供生命周期关联——所有安全性均由调用者负责。
/// 适合在需要绕过 Rust 所有权/借用检查，但又希望保持一定类型一致性的场景（如 FFI 或底层跨线程传递）。
///
/// # 安全性总览
/// **该类型的所有方法均为 `unsafe`**，因为任何对裸指针的解引用、转换或地址操作都可能导致未定义行为。
/// 使用前，调用者**必须**确保：
///
/// * 指针所指向的内存区域在解引用时是**有效**的（已分配、未释放、且对齐正确）。
/// * 若进行可变操作（`as_mut_ptr`、`deref_mut`），则必须保证不存在其它并发访问（包括读写），
///   除非使用了外部同步机制（如 `Mutex`、`RwLock`、原子操作等）。
/// * 若跨线程传递，调用者需自行保证数据在线程间的**同步**，避免数据竞争和悬垂指针。
/// * 指针不可为空（`null`），除非调用者明确将其用作标记且从不解引用。
/// * 生命周期必须由调用者手动维护：确保在解引用时原始数据依然存活，且未发生 `move` 或 `drop`。
/// * 泛型参数 `T` 必须与指针实际指向的类型完全匹配，否则转换或解引用将导致立即未定义行为。
///
/// # `Send` 与 `Sync`
/// 该类型本身仅含有一个指针（机器字长），因此类型层面可自动满足 `Send` 和 `Sync`。
/// 但**这绝不意味着所指向的数据也是 `Send`/`Sync`**。调用者必须确保跨线程使用时，
/// 指针指向的数据符合相应的线程安全要求（例如使用 `Arc<Mutex<T>>` 或确保 `T: Send + Sync`）。
///
/// # 示例
/// ```
/// use std::thread;
/// use array_viewer::unsafe_pointer::UnsafePointerHandler;
///
/// let mut value = 42;
/// // 从可变指针创建处理器
/// let handler = unsafe { UnsafePointerHandler::from_mut_ptr(&mut value) };
///
/// let handle = thread::spawn(move || {
///     unsafe {
///         // 在新线程中获取可变引用并修改
///         let mut_ref = handler.deref_mut();
///         *mut_ref += 1;
///     }
/// });
/// handle.join().unwrap();
///
/// // 主线程等待子线程结束，确保 value 在访问期间存活
/// assert_eq!(value, 43);
///
/// // 注意：若子线程在 value 生命周期外访问，将导致悬垂指针，调用者需自行避免。
/// ```
pub struct UnsafePointerHandler<T: ?Sized>(pub *mut T);

// 显式实现 Send 和 Sync（虽然编译器可能自动推导，但我们明确声明以强调意图）
// 安全性：该包装本身仅包含一个指针，可安全在线程间传递，但数据安全性由调用者保证。
unsafe impl<T: ?Sized> Send for UnsafePointerHandler<T> {}
unsafe impl<T: ?Sized> Sync for UnsafePointerHandler<T> {}

impl<T: ?Sized> UnsafePointerHandler<T> {
    /// 从不可变裸指针创建处理器（内部转换为可变指针存储）。
    ///
    /// # Safety
    /// 调用者必须保证 `ptr` 非空、有效且符合所有后续使用的安全性条件。
    /// 即使最初为 `*const T`，存储为 `*mut T` 并不会自动允许写操作，写操作仍需满足别名规则。
    #[inline]
    pub unsafe fn from_ptr(ptr: *const T) -> Self {
        Self(ptr as *mut T)
    }

    /// 从可变裸指针创建处理器。
    ///
    /// # Safety
    /// 同 `from_ptr`，且额外要求该指针允许可变访问且无其他别名。
    #[inline]
    pub unsafe fn from_mut_ptr(ptr: *mut T) -> Self {
        Self(ptr)
    }

    /// 获取存储的不可变指针（`*const T`）。
    ///
    /// # Safety
    /// 返回的指针本身未经验证，调用者必须确保其有效，且不可用于可变访问（除非额外确保独占性）。
    #[inline]
    pub unsafe fn as_ptr(&self) -> *const T {
        self.0 as *const T
    }

    /// 获取存储的可变指针（`*mut T`）。
    ///
    /// # Safety
    /// 返回的指针未经验证，调用者必须确保该指针有效且满足独占可变访问的条件（无并发读写）。
    #[inline]
    pub unsafe fn as_mut_ptr(&self) -> *mut T {
        self.0
    }

    /// 不可变地解引用存储的指针，得到 `&'a T`。
    ///
    /// # Safety
    /// * 指针必须非空且指向一个初始化的 `T` 实例。
    /// * 该实例在返回引用的生命周期 `'a` 内必须保持有效且不被修改（除非使用内部可变性）。
    /// * 不存在任何并发可变引用。
    #[inline]
    pub unsafe fn deref<'a>(&self) -> &'a T {
        unsafe { &*self.0 }
    }

    /// 可变地解引用存储的指针，得到 `&'a mut T`。
    ///
    /// # Safety
    /// * 指针必须非空且指向一个初始化的 `T` 实例。
    /// * 在返回引用的生命周期 `'a` 内，该实例必须独占访问，不存在任何其它引用（包括不可变引用）。
    /// * 确保没有其它线程同时访问该数据（除非外部同步机制保证）。
    #[inline]
    pub unsafe fn deref_mut<'a>(&self) -> &'a mut T {
        unsafe { &mut *self.0 }
    }

    /// 消费处理器，返回存储的可变裸指针。
    ///
    /// # Safety
    /// 返回的指针不再受此包装约束，调用者需自行承担后续所有使用责任，确保有效性。
    #[inline]
    pub unsafe fn into_mut_ptr(self) -> *mut T {
        self.0
    }

    /// 消费处理器，返回存储的不可变裸指针。
    ///
    /// # Safety
    /// 同上，返回的指针不再受包装约束，调用者需保证后续使用安全。
    #[inline]
    pub unsafe fn into_ptr(self) -> *const T {
        self.0 as *const T
    }
}

impl<T> UnsafePointerHandler<T> {
    /// 从 `usize` 地址值创建处理器，强制转换为 `*mut T`。
    ///
    /// # Safety
    /// 调用者必须确保该地址确实是一个指向 `T` 的有效指针地址，且满足所有对齐、有效性条件。
    /// 此方法绕过了类型检查，风险极高，推荐仅在确知地址来源时使用。
    #[inline]
    pub unsafe fn from_usize(addr: usize) -> Self {
        Self(addr as *mut T)
    }

    /// 取出内部存储的 `usize` 地址值。
    ///
    /// # Safety
    /// 此方法本身仅读取地址，但该地址后续可能被误用。调用者应意识到返回的地址可能已失效，
    /// 任何后续将其转回指针并解引用的操作都必须重新满足所有安全条件。
    #[inline]
    pub unsafe fn into_usize(self) -> usize {
        self.0 as usize
    }

    /// 创建一个指向空（null）的处理器。
    ///
    /// # Safety
    /// 创建空指针本身是安全的，但任何对该指针的解引用操作都将导致未定义行为。
    /// 此方法仅适用于需要显式表示“空”标记的场景，且调用者必须确保从不解引用它。
    #[inline]
    pub unsafe fn null() -> Self {
        Self(ptr::null_mut())
    }
}
