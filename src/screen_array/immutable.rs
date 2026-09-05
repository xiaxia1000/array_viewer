//! 定义固定尺寸的二维像素缓冲区句柄，内存位于堆上且地址固定。

#[cfg(feature = "array_from_image")]
use image::DynamicImage;
#[cfg(feature = "array_from_image")]
use image::GenericImageView;
#[cfg(feature = "array_from_image")]
use std::path::Path;
use std::ptr;

/// 一个**只持有指针**的二维屏幕缓冲句柄。
///
/// # 内存语义（非常重要）
/// - `ScreenArray` **自身可以移动**（即该结构体可以复制、传递）。
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
/// 该类型实现了 `Send` 和 `Sync`，因此可以安全地在线程间共享引用，
/// 但指针所指内容的并发访问需要外部同步。
#[derive(Debug)]
#[repr(transparent)]
pub struct ScreenArray<const W: usize, const H: usize> {
    /// 指向堆上二维数组的原始指针
    ///
    /// 该指针：
    /// - 来自 `Box::into_raw`
    /// - 只能通过 `Box::from_raw` 回收
    arr_ptr: *mut [[u32; W]; H],
}

impl<const W: usize, const H: usize> ScreenArray<W, H> {
    const _CHECK: () = {
        let _ = W.checked_mul(H).expect("[ScreenArray<W,H>] W * H overflows usize");
    };

    /// 从栈上的二维数组构造一个固定地址的堆缓冲。
    ///
    /// # 行为
    /// - 将 `[[u32; W]; H]` 从栈移动到堆。
    /// - 放弃 Rust 的自动 drop 权（内存由 `ScreenArray` 管理）。
    /// - 返回一个“裸句柄”。
    ///
    /// # 注意
    /// 新分配的堆内存地址是固定的，直到调用 `drop()`。
    pub fn new(array: [[u32; W]; H]) -> Self {
        let arr_box = Box::new(array);
        let arr_ptr = Box::into_raw(arr_box);
        Self {
            arr_ptr,
        }
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
        let boxed = Box::<[[u32; W]; H]>::new_uninit();

        // 2. 强行声明“已初始化”
        // 注意：内存内容仍然是未定义的
        let arr_box = unsafe { boxed.assume_init() };
        let arr_ptr = Box::into_raw(arr_box);

        Self {
            arr_ptr,
        }
    }

    /// 在堆上分配二维缓冲，并将其**按字节清零**。
    ///
    /// 该方法：
    /// - 调用 `new_uninit` 获取未初始化缓冲。
    /// - 使用 `write_bytes` 将整块内存置零。
    ///
    /// # 结果
    /// - 返回的缓冲是**完全零初始化**的。
    /// - 可直接安全使用（像素值为 0x00000000）。
    pub fn zero() -> Self {
        let this = Self::new_uninit();

        unsafe {
            let ptr = this.arr_ptr as *mut u8;
            let size = size_of::<[[u32; W]; H]>();
            ptr::write_bytes(ptr, 0, size);
        }

        this
    }

    /// 从原始指针创建 `ScreenArray`（指针指向 `[[u32; W]; H]` 类型）。
    ///
    /// # Safety
    /// - 指针必须指向一块有效的、大小为 `size_of::<[[u32; W]; H]>()` 的堆内存
    /// - 该内存必须是由 `Box::into_raw` 或类似方式分配的
    /// - 调用者必须确保不会发生 double-free
    /// - 指针不能为 null
    pub unsafe fn from_raw(arr_ptr: *mut [[u32; W]; H]) -> Self {
        assert!(!arr_ptr.is_null(), "[ScreenArray<W,H>::from_raw] null pointer");

        Self {
            arr_ptr,
        }
    }

    /// 手动释放堆内存。
    ///
    /// # Safety
    /// - 只能调用一次。
    /// - 调用后不能再访问任何 `get` / `as_slice` 等方法。
    pub fn drop(mut self) {
        unsafe {
            let _ = Box::from_raw(self.arr_ptr);
            self.arr_ptr = ptr::null_mut();
        }
    }

    /// 脱离 `ScreenArray` 对象，仅通过指针释放内存。
    ///
    /// 用于极端场景（例如跨 FFI / 自定义资源管理器），
    /// 此时可以不需要持有 `ScreenArray` 实例。
    ///
    /// # Safety
    /// - 传入的指针必须是由 `ScreenArray` 分配的。
    /// - 不能重复释放。
    pub unsafe fn drop_it(arr_ptr: *mut [[u32; W]; H]) {
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
    pub fn get_mut(&self) -> &mut [[u32; W]; H] {
        unsafe { &mut *self.arr_ptr }
    }

    /// 获取内部二维数组的不可变引用。
    pub fn get(&self) -> &[[u32; W]; H] {
        unsafe { &*self.arr_ptr }
    }

    /// 直接通过坐标访问像素（可变）。
    ///
    /// # Safety
    /// - 坐标必须合法（`x < W && y < H`）。
    /// - 不得造成数据竞争。
    pub unsafe fn get_from_index_mut(&self, x: usize, y: usize) -> &mut u32 {
        unsafe {
            let arr: &mut [[u32; W]; H] = &mut *self.arr_ptr;
            &mut arr[y][x]
        }
    }

    /// 直接通过坐标访问像素（不可变）。
    pub unsafe fn get_from_index(&self, x: usize, y: usize) -> &u32 {
        unsafe {
            let arr: &[[u32; W]; H] = &*self.arr_ptr;
            &arr[y][x]
        }
    }

    /// 返回原始指针（用于 FFI / GPU / mmap）。
    pub fn get_ptr(&self) -> *mut [[u32; W]; H] {
        self.arr_ptr
    }

    /// 返回指针的数值形式。
    ///
    /// 极度危险，仅用于调试、FFI 或底层资源管理。
    pub unsafe fn get_ptr_num(&self) -> usize {
        self.arr_ptr as usize
    }

    /// 返回指定像素的原始指针。
    pub unsafe fn get_from_index_ptr(&self, x: usize, y: usize) -> *mut u32 {
        unsafe { &mut (*self.arr_ptr)[y][x] as *mut u32 }
    }

    /// 完全脱离对象的指针访问（静态函数）。
    ///
    /// 用于“只有地址，没有对象”的极端场景。
    pub unsafe fn get_from_index_ptr_selfless(self_ptr: usize, x: usize, y: usize) -> *mut u32 {
        unsafe {
            let arr = self_ptr as *mut [[u32; W]; H];
            &mut (*arr)[y][x]
        }
    }

    /// 返回**行优先、连续**的一维切片。
    ///
    /// 该布局与 C / GPU / framebuffer 完全兼容。
    pub fn as_slice(&self) -> &[u32] {
        unsafe {
            let ptr = self.arr_ptr as *const u32;
            std::slice::from_raw_parts(ptr, W * H)
        }
    }

    /// 返回可变的一维切片。
    pub fn as_mut_slice(&self) -> &mut [u32] {
        unsafe {
            let ptr = self.arr_ptr as *mut u32;
            std::slice::from_raw_parts_mut(ptr, W * H)
        }
    }
}

#[cfg(feature = "array_from_image")]
impl<const W: usize, const H: usize> ScreenArray<W, H> {
    /// 从指定路径加载图像，并将其像素数据写入当前 `ScreenArray` 实例。
    ///
    /// # 参数
    /// - `path`: 图像文件路径（任何实现 `AsRef<Path>` 的类型）。
    ///
    /// # 返回
    /// - `Ok(())` 如果图像成功加载且尺寸匹配。
    /// - `Err(Box<dyn Error>)` 如果文件打开失败、解码失败或尺寸不匹配。
    ///
    /// # 像素格式
    /// 内部存储为 `u32`，格式为 **ARGB**（最高字节 Alpha，然后依次为 Red、Green、Blue）。
    ///
    /// # 注意事项
    /// - 图像必须为 `W × H` 像素，否则返回错误。
    /// - 此方法使用不安全操作直接写入内存，调用者需确保 `ScreenArray` 内存有效。
    pub fn set_image<P: AsRef<Path>>(&mut self, path: P)
                                     -> Result<(), Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let img = image::open(path)?;
        let (width, height) = img.dimensions();
        if width as usize != W || height as usize != H {
            return Err("[ScreenArray<W,H>::set_image] image dimensions do not match".into());
        }
        let raw = img.into_rgba8().into_raw();

        for i in 0..H * W {
            let r = raw[i * 4] as u32;
            let g = raw[i * 4 + 1] as u32;
            let b = raw[i * 4 + 2] as u32;
            let a = raw[i * 4 + 3] as u32;
            unsafe {
                *self.get_from_index_mut(i % W, i / W) = a << 24 | r << 16 | g << 8 | b;
            }
        }

        Ok(())
    }

    /// 从指定路径加载图像，并返回一个新的 `ScreenArray` 实例，其中包含图像的像素数据。
    ///
    /// # 参数
    /// - `path`: 图像文件路径。
    ///
    /// # 返回
    /// - `Ok(ScreenArray<W, H>)` 如果图像加载成功且尺寸匹配。
    /// - `Err(Box<dyn Error>)` 如果文件打开、解码失败或尺寸不匹配。
    ///
    /// # 像素格式
    /// 与 [`set_image`] 相同，为 ARGB 格式的 `u32`。
    pub fn from_image<P: AsRef<Path>>(path: P)
                                      -> Result<ScreenArray<W, H>, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let img = image::open(path)?;
        Self::try_from(img)
    }
}

#[cfg(feature = "array_from_image")]
impl<const W: usize, const H: usize> TryFrom<DynamicImage> for ScreenArray<W, H> {

    type Error = Box<dyn std::error::Error>;

    fn try_from(value: DynamicImage) -> Result<Self, Self::Error> {
        let (width, height) = value.dimensions();
        if width as usize != W || height as usize != H {
            return Err("[ScreenArray<W,H>::try_from] image dimensions do not match".into());
        }
        let raw = value.into_rgba8().into_raw();

        let ptr: Vec<u32> = raw
            .chunks_exact(4)
            .map(|chunk| {
                let r = chunk[0] as u32;
                let g = chunk[1] as u32;
                let b = chunk[2] as u32;
                let a = chunk[3] as u32;
                a << 24 | r << 16 | g << 8 | b
            })
            .collect();

        Ok(ScreenArray {
            arr_ptr: ptr.into_raw_parts().0 as *mut [[u32; W]; H],
        })
    }
}


/// 默认构造为零缓冲。
impl<const W: usize, const H: usize> Default for ScreenArray<W, H> {
    fn default() -> Self {
        Self::zero()
    }
}

// 允许跨线程传递原始指针（但需外部同步访问内容）。
unsafe impl<const W: usize, const H: usize> Send for ScreenArray<W, H> {}
unsafe impl<const W: usize, const H: usize> Sync for ScreenArray<W, H> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_and_size() {
        let arr = ScreenArray::<4, 3>::zero();
        assert_eq!(arr.as_slice(), &[0u32; 12]);
        assert_eq!(ScreenArray::<4, 3>::size(), (3, 4));
        arr.drop();
    }

    #[test]
    fn new_and_read_write() {
        let arr = ScreenArray::<4, 3>::new([[1u32; 4]; 3]);
        assert_eq!(arr.get()[0][0], 1);
        assert_eq!(arr.get()[2][3], 1);

        // 通过一维可变切片写入，再经二维视图/坐标读取
        arr.as_mut_slice()[5] = 42;
        assert_eq!(arr.as_slice()[5], 42);
        unsafe {
            assert_eq!(*arr.get_from_index(1, 1), 42);
            *arr.get_from_index_mut(0, 0) = 7;
            assert_eq!(*arr.get_from_index(0, 0), 7);
        }
        arr.drop();
    }

    #[test]
    fn init_pairs() {
        let (arr, _viewer) = crate::init::<16, 16>();
        arr.as_mut_slice().fill(0xFFFF_0000);
        assert_eq!(arr.as_slice()[0], 0xFFFF_0000);
        assert_eq!(arr.as_slice().len(), 16 * 16);
        arr.drop();
    }
}