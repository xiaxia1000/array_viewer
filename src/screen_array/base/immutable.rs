//! 定义固定尺寸的二维像素缓冲区句柄，内存位于堆上且地址固定。
//!
//! 泛型参数 `T` 表示像素类型，需满足 `Sized` 且通常还应实现 `Clone`、`Send`、`Sync` 等。
//! `zero()` 方法额外要求 `T: num_traits::Zero`。

#[cfg(feature = "array_from_image")]
use image::DynamicImage;
#[cfg(feature = "array_from_image")]
use image::GenericImageView;
#[cfg(feature = "array_from_image")]
use std::path::Path;

/// 一个**只持有指针**的二维屏幕缓冲句柄。
///
/// # 泛型参数
/// - `T`: 像素类型。需满足 `Sized`，且通常实现 `Clone`、`Send`、`Sync`。
/// - `W`: 屏幕宽度（列数）。
/// - `H`: 屏幕高度（行数）。
///
/// # 内存语义（非常重要）
/// - `ScreenArrayBase` **自身可以移动**（即该结构体可以复制、传递）。
/// - 内部指针 `arr_ptr` 指向的堆内存地址永久固定（直到手动释放），
///   不受 Rust 生命周期系统管理。
///
/// # 安全契约
/// - 该内存必须由使用者**手动释放**（调用 `drop()` 或 `drop_it()`）。
/// - 不能 double-free。
/// - 不能在释放后继续访问任何 `get` / `as_slice` 方法。
///
/// # 用途
/// 该结构常用于：
/// - framebuffer
/// - 显存映射
/// - 与 C / FFI / GPU 交换稳定地址
///
/// # 线程安全
/// 当 `T: Send + Sync` 时，该类型实现了 `Send` 和 `Sync`，可以安全地在线程间共享引用，
/// 但指针所指内容的并发访问需要外部同步。
#[derive(Debug)]
#[repr(transparent)]
pub struct ScreenArrayBase<T, const W: usize, const H: usize> {
    /// 指向堆上二维数组的原始指针
    ///
    /// 该指针：
    /// - 来自 `Box::into_raw`
    /// - 只能通过 `Box::from_raw` 回收
    arr_ptr: *mut [[T; W]; H],
}

// 主要实现块：不要求 `T` 实现特殊 trait（仅需 `Sized`）
impl<T, const W: usize, const H: usize> ScreenArrayBase<T, W, H> {
    const _CHECK: () = {
        let _ = W.checked_mul(H).expect("[ScreenArrayBase<T,W,H>] W * H overflows usize");
    };

    /// 从栈上的二维数组构造一个固定地址的堆缓冲。
    ///
    /// # 行为
    /// - 将 `[[T; W]; H]` 从栈移动到堆。
    /// - 放弃 Rust 的自动 drop 权（内存由 `ScreenArrayBase` 管理）。
    /// - 返回一个“裸句柄”。
    ///
    /// # 注意
    /// 新分配的堆内存地址是固定的，直到调用 `drop()`。
    pub fn new(array: [[T; W]; H]) -> Self {
        let arr_box = Box::new(array);
        let arr_ptr = Box::into_raw(arr_box);
        Self { arr_ptr }
    }

    /// 在堆上分配二维缓冲，但**不进行任何初始化**。
    ///
    /// # 内存状态
    /// - 内存已分配（堆上）。
    /// - 内容未定义（UB 如果直接读取）。
    ///
    /// # Safety
    /// - 调用者必须在使用前完成初始化（例如通过 `as_mut_slice()` 写入）。
    /// - 否则任何读取都会触发未定义行为。
    pub fn new_uninit() -> Self {
        // 1. 分配未初始化内存
        let boxed = Box::<[[T; W]; H]>::new_uninit();

        // 2. 强行声明“已初始化”
        // 注意：内存内容仍然是未定义的
        let arr_box = unsafe { boxed.assume_init() };
        let arr_ptr = Box::into_raw(arr_box);

        Self { arr_ptr }
    }

    /// 从原始指针创建 `ScreenArrayBase`（指针指向 `[[T; W]; H]` 类型）。
    ///
    /// # Safety
    /// - 指针必须指向一块有效的、大小为 `size_of::<[[T; W]; H]>()` 的堆内存
    /// - 该内存必须是由 `Box::into_raw` 或类似方式分配的
    /// - 调用者必须确保不会发生 double-free
    /// - 指针不能为 null
    pub unsafe fn from_raw(arr_ptr: *mut [[T; W]; H]) -> Self {
        assert!(
            !arr_ptr.is_null(),
            "[ScreenArrayBase<T,W,H>::from_raw] null pointer"
        );
        Self { arr_ptr }
    }

    /// 手动释放堆内存。
    ///
    /// # Safety
    /// - 只能调用一次。
    /// - 调用后不能再访问任何 `get` / `as_slice` 等方法。
    pub fn drop(self) {
        unsafe {
            let _ = Box::from_raw(self.arr_ptr);
        }
    }

    /// 脱离 `ScreenArrayBase` 对象，仅通过指针释放内存。
    ///
    /// 用于极端场景（例如跨 FFI / 自定义资源管理器），
    /// 此时可以不需要持有 `ScreenArrayBase` 实例。
    ///
    /// # Safety
    /// - 传入的指针必须是由 `ScreenArrayBase` 分配的。
    /// - 不能重复释放。
    pub unsafe fn drop_it(arr_ptr: *mut [[T; W]; H]) {
        unsafe {
            let _ = Box::from_raw(arr_ptr);
        }
    }

    /// 返回屏幕分辨率（高 × 宽）。
    pub const fn size() -> (usize, usize) {
        (H, W)
    }

    /// 获取内部二维数组的可变引用。
    ///
    /// # Safety
    /// - 调用者必须保证独占访问（无其他线程同时修改）。
    pub fn get_mut(&self) -> &mut [[T; W]; H] {
        unsafe { &mut *self.arr_ptr }
    }

    /// 获取内部二维数组的不可变引用。
    pub fn get(&self) -> &[[T; W]; H] {
        unsafe { &*self.arr_ptr }
    }

    /// 直接通过坐标访问像素（可变）。
    ///
    /// # Safety
    /// - 坐标必须合法（`x < W && y < H`）。
    /// - 不得造成数据竞争。
    pub unsafe fn get_from_index_mut(&self, x: usize, y: usize) -> &mut T {
        unsafe {
            let arr: &mut [[T; W]; H] = &mut *self.arr_ptr;
            &mut arr[y][x]
        }
    }

    /// 直接通过坐标访问像素（不可变）。
    ///
    /// # Safety
    /// - 坐标必须合法（`x < W && y < H`）。
    /// - 不得造成数据竞争。
    pub unsafe fn get_from_index(&self, x: usize, y: usize) -> &T {
        unsafe {
            let arr: &[[T; W]; H] = &*self.arr_ptr;
            &arr[y][x]
        }
    }

    /// 返回原始指针（用于 FFI / GPU / mmap）。
    pub fn get_ptr(&self) -> *mut [[T; W]; H] {
        self.arr_ptr
    }

    /// 返回指针的数值形式。
    ///
    /// 极度危险，仅用于调试、FFI 或底层资源管理。
    pub unsafe fn get_ptr_num(&self) -> usize {
        self.arr_ptr as usize
    }

    /// 返回指定像素的原始指针。
    ///
    /// # Safety
    /// - 坐标必须合法（`x < W && y < H`）。
    /// - 不得造成数据竞争。
    pub unsafe fn get_from_index_ptr(&self, x: usize, y: usize) -> *mut T {
        unsafe { &mut (*self.arr_ptr)[y][x] as *mut T }
    }

    /// 完全脱离对象的指针访问（静态函数）。
    ///
    /// 用于“只有地址，没有对象”的极端场景。
    ///
    /// # Safety
    /// - 坐标必须合法（`x < W && y < H`）。
    /// - 不得造成数据竞争。
    pub unsafe fn get_from_index_ptr_selfless(self_ptr: usize, x: usize, y: usize) -> *mut T {
        unsafe {
            let arr = self_ptr as *mut [[T; W]; H];
            &mut (*arr)[y][x]
        }
    }

    /// 返回**行优先、连续**的一维切片。
    ///
    /// 该布局与 C / GPU / framebuffer 完全兼容。
    pub fn as_slice(&self) -> &[T] {
        unsafe {
            let ptr = self.arr_ptr as *const T;
            std::slice::from_raw_parts(ptr, W * H)
        }
    }

    /// 返回可变的一维切片。
    pub fn as_mut_slice(&self) -> &mut [T] {
        unsafe {
            let ptr = self.arr_ptr as *mut T;
            std::slice::from_raw_parts_mut(ptr, W * H)
        }
    }
}

// 零初始化实现块：要求 `T: Zero`
impl<T: num_traits::Zero, const W: usize, const H: usize> ScreenArrayBase<T, W, H> {
    /// 在堆上分配二维缓冲，并将其**按元素初始化为 `T::zero()`**。
    ///
    /// 该方法：
    /// - 调用 `new_uninit` 获取未初始化缓冲。
    /// - 遍历每个元素，调用 `T::zero()` 进行赋值。
    ///
    /// # 结果
    /// - 返回的缓冲是**完全零初始化**的。
    /// - 可直接安全使用（所有像素值为 `T::zero()`）。
    pub fn zero() -> Self {
        let this = Self::new_uninit();
        // 安全：我们获得了独占的可变切片，且每个元素都通过 `T::zero()` 初始化
        let slice = this.as_mut_slice();
        for elem in slice.iter_mut() {
            *elem = T::zero();
        }
        this
    }
}

// 默认实现：使用 `Zero` 进行零初始化
impl<T: num_traits::Zero, const W: usize, const H: usize> Default for ScreenArrayBase<T, W, H> {
    fn default() -> Self {
        Self::zero()
    }
}

// 安全 trait 实现：需要像素类型满足 `Send` 和 `Sync`
unsafe impl<T: Send, const W: usize, const H: usize> Send for ScreenArrayBase<T, W, H> {}
unsafe impl<T: Sync, const W: usize, const H: usize> Sync for ScreenArrayBase<T, W, H> {}

// 图像加载功能（需要启用 `array_from_image` 特性）
#[cfg(feature = "array_from_image")]
impl<T, const W: usize, const H: usize> ScreenArrayBase<T, W, H> {
    /// 从指定路径加载图像，并使用提供的转换闭包将每个像素转换为 `T` 后写入当前实例。
    ///
    /// # 参数
    /// - `path`: 图像文件路径。
    /// - `converter`: 一个闭包，接收打包为 `u32` 的像素值（格式：0xAARRGGBB），返回 `T`。
    ///
    /// # 返回
    /// - `Ok(())` 如果图像成功加载且尺寸匹配。
    /// - `Err(Box<dyn Error>)` 如果文件打开失败、解码失败或尺寸不匹配。
    ///
    /// # 像素格式
    /// - 输入图像会被解码为 RGBA8，然后打包为 `u32`（最高字节为 Alpha，依次为 R、G、B）。
    /// - 转换闭包负责将 `u32` 解释为所需的 `T` 类型。
    pub fn set_image<P: AsRef<Path>, F: Fn(u32) -> T>(
        &mut self,
        path: P,
        converter: F,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let img = image::open(path)?;
        let (width, height) = img.dimensions();
        if width as usize != W || height as usize != H {
            return Err("[ScreenArrayBase<T,W,H>::set_image] image dimensions do not match".into());
        }
        let raw = img.into_rgba8().into_raw();

        // 逐像素转换并写入
        for i in 0..H * W {
            let r = raw[i * 4] as u32;
            let g = raw[i * 4 + 1] as u32;
            let b = raw[i * 4 + 2] as u32;
            let a = raw[i * 4 + 3] as u32;
            let packed = a << 24 | r << 16 | g << 8 | b;
            let pixel = converter(packed);
            unsafe {
                *self.get_from_index_mut(i % W, i / W) = pixel;
            }
        }

        Ok(())
    }

    /// 从指定路径加载图像，并使用提供的转换闭包将每个像素转换为 `T`，返回一个新的 `ScreenArrayBase`。
    ///
    /// # 参数
    /// - `path`: 图像文件路径。
    /// - `converter`: 一个闭包，接收打包为 `u32` 的像素值（格式：0xAARRGGBB），返回 `T`。
    ///
    /// # 返回
    /// - `Ok(ScreenArrayBase<T, W, H>)` 如果图像加载成功且尺寸匹配。
    /// - `Err(Box<dyn Error>)` 如果文件打开、解码失败或尺寸不匹配。
    pub fn from_image<P: AsRef<Path>, F: Fn(u32) -> T>(
        path: P,
        converter: F,
    ) -> Result<ScreenArrayBase<T, W, H>, Box<dyn std::error::Error>> {
        let mut arr = ScreenArrayBase::new_uninit();
        arr.set_image(path, converter)?;
        Ok(arr)
    }
}

// 可选的 `TryFrom<DynamicImage>` 实现：要求 `T: From<u32>` 作为便捷方式
#[cfg(feature = "array_from_image")]
impl<T: From<u32>, const W: usize, const H: usize> TryFrom<DynamicImage> for ScreenArrayBase<T, W, H> {
    type Error = Box<dyn std::error::Error>;

    fn try_from(value: DynamicImage) -> Result<Self, Self::Error> {
        let (width, height) = value.dimensions();
        if width as usize != W || height as usize != H {
            return Err("[ScreenArrayBase<T,W,H>::try_from] image dimensions do not match".into());
        }
        let raw = value.into_rgba8().into_raw();

        let pixels: Vec<T> = try_from_image_inner(raw);

        // 构造 ScreenArrayBase：将 Vec<T> 的底层指针重新解释为二维数组指针
        // 注意：需要确保 W * H == pixels.len()
        let ptr = pixels.leak().as_mut_ptr() as *mut [[T; W]; H];
        Ok(unsafe { ScreenArrayBase::from_raw(ptr) })
    }
}

#[cfg(feature = "array_from_image")]
pub(super) fn try_from_image_inner<T: From<u32>>(raw: Vec<u8>) -> Vec<T> {
    // 将每个像素打包为 u32 并通过 `T::from` 转换
    raw
        .chunks_exact(4)
        .map(|chunk| {
            let r = chunk[0] as u32;
            let g = chunk[1] as u32;
            let b = chunk[2] as u32;
            let a = chunk[3] as u32;
            let packed = a << 24 | r << 16 | g << 8 | b;
            T::from(packed)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::ops::Add;
    use super::*;

    #[test]
    fn zero_and_size() {
        let arr = ScreenArrayBase::<u32, 4, 3>::zero();
        assert_eq!(arr.as_slice(), &[0u32; 12]);
        assert_eq!(ScreenArrayBase::<u32, 4, 3>::size(), (3, 4));
        arr.drop();
    }

    #[test]
    fn new_and_read_write() {
        let arr = ScreenArrayBase::<u32, 4, 3>::new([[1u32; 4]; 3]);
        assert_eq!(arr.get()[0][0], 1);
        assert_eq!(arr.get()[2][3], 1);

        arr.as_mut_slice()[5] = 42;
        assert_eq!(arr.as_slice()[5], 42);
        unsafe {
            assert_eq!(*arr.get_from_index(1, 1), 42);
            *arr.get_from_index_mut(0, 0) = 7;
            assert_eq!(*arr.get_from_index(0, 0), 7);
        }
        arr.drop();
    }

    // 测试使用自定义像素类型（非 u32）
    #[derive(Clone, Debug, PartialEq)]
    #[repr(transparent)]
    struct CustomPixel(u8);

    impl Add<Self> for CustomPixel {
        type Output = Self;

        fn add(self, rhs: Self) -> Self::Output {
            CustomPixel(self.0 + rhs.0)
        }
    }

    impl num_traits::Zero for CustomPixel {
        fn zero() -> Self {
            CustomPixel(0)
        }
        fn is_zero(&self) -> bool {
            self.0 == 0
        }
    }

    #[test]
    fn generic_custom_type() {
        let arr = ScreenArrayBase::<CustomPixel, 2, 2>::zero();
        assert_eq!(arr.as_slice(), &[CustomPixel(0), CustomPixel(0), CustomPixel(0), CustomPixel(0)]);
        arr.drop();

        let arr2 = ScreenArrayBase::<CustomPixel, 2, 2>::new([
            [CustomPixel(1), CustomPixel(2)],
            [CustomPixel(3), CustomPixel(4)],
        ]);
        assert_eq!(arr2.get()[1][0], CustomPixel(3));
        arr2.drop();
    }
}