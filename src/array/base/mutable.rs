//! 定义动态尺寸的二维像素缓冲区句柄，内存位于堆上且地址固定。
//!
//! 泛型参数 `T` 表示像素类型，必须实现 `num_traits::Zero` 和 `Copy`。
//! `Zero` 用于零初始化，`Copy` 用于安全的按位拷贝（像素类型通常为简单数值）。
//!
//! # 内存语义
//! - `ScreenArrayBase` 自身可以移动。
//! - 内部指针 `arr_ptr` 指向的堆内存地址固定，直到被手动释放。
//! - 内存必须通过 `drop()` 或 `drop_it()` 手动释放，不能 double-free。
//!
//! # 安全契约
//! - 释放后不能再访问任何 `get` / `as_slice` 方法。
//! - 使用 `set_size_unchecked` 等 unsafe 方法时必须保证内存安全。
//!
//! # 线程安全
//! 当 `T: Send + Sync` 时，该类型实现了 `Send` 和 `Sync`，但并发访问内容需要外部同步。

#[cfg(feature = "array_from_image")]
use image::GenericImageView;
#[cfg(feature = "array_from_image")]
use std::path::Path;
use std::alloc::{alloc, dealloc, handle_alloc_error, Layout};
use std::ptr;
#[cfg(feature = "array_from_image")]
use crate::base::immutable::try_from_image_inner;

/// 动态尺寸的二维像素缓冲区句柄，内存位于堆上且地址固定。
#[derive(Debug)]
pub struct ScreenArrayBase<T: num_traits::Zero + Copy> {
    /// 指向连续 T 缓冲区的原始指针
    arr_ptr: *mut T,
    /// 当前宽度（像素列数）
    width: usize,
    /// 当前高度（像素行数）
    height: usize,
}

// -------------------------------------------------------------------------------------------------
// 内部辅助函数：内存分配与释放
// -------------------------------------------------------------------------------------------------

/// 分配 `len` 个 `T` 的未初始化内存，返回指针。
/// 若 `len == 0` 则返回空指针。
fn alloc_buffer<T>(len: usize) -> *mut T {
    if len == 0 {
        return ptr::null_mut();
    }
    let layout = Layout::array::<T>(len).expect("[ScreenArrayBaseBase<T>] layout overflow");
    unsafe {
        let ptr = alloc(layout);
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        ptr as *mut T
    }
}

/// 释放由 `alloc_buffer` 分配的 `len` 个 `T` 内存。
///
/// # Safety
/// - `ptr` 必须是由 `alloc_buffer` 分配且未被释放过的指针。
/// - `len` 必须与分配时的大小一致。
unsafe fn dealloc_buffer<T>(ptr: *mut T, len: usize) {
    if len == 0 || ptr.is_null() {
        return;
    }
    let layout = Layout::array::<T>(len).unwrap();
    unsafe {
        dealloc(ptr as *mut u8, layout);
    }
}

// -------------------------------------------------------------------------------------------------
// 二维视图类型（用于 `get` 和 `get_mut` 的返回）
// -------------------------------------------------------------------------------------------------

/// 生成 width() 和 height() 方法
macro_rules! impl_screen_size_methods {
    ($width:ident, $slice:ident) => {
        #[inline]
        pub fn width(&self) -> usize {
            self.$width
        }

        #[inline]
        pub fn height(&self) -> usize {
            if self.$width == 0 {
                0
            } else {
                debug_assert_eq!(self.$slice.len() % self.width(), 0);
                self.$slice.len() / self.$width
            }
        }
    };
}

/// 生成 get() 方法
macro_rules! impl_screen_get {
    ($width:ident, $height:ident, $slice:ident) => {
        pub fn get(&self, x: usize, y: usize) -> Option<&T> {
            if x < self.$width && y < self.$height() {
                // SAFETY: bounds checked above
                Some(&self.$slice[y * self.$width + x])
            } else {
                None
            }
        }
    };
}

/// 不可变二维视图，提供 `view[y][x]` 访问。
#[derive(Debug, Clone, Copy)]
pub struct ScreenArrayBaseView<'a, T: num_traits::Zero + Copy> {
    slice: &'a [T],
    width: usize,
}

impl<'a, T: num_traits::Zero + Copy> ScreenArrayBaseView<'a, T> {
    impl_screen_size_methods!(width, slice);
    impl_screen_get!(width, height, slice);
}

impl<'a, T: num_traits::Zero + Copy> std::ops::Index<usize> for ScreenArrayBaseView<'a, T> {
    type Output = [T];

    fn index(&self, row: usize) -> &Self::Output {
        let start = row
            .checked_mul(self.width)
            .expect("[ScreenArrayBaseView::index] overflow");
        &self.slice[start..start + self.width]
    }
}

/// 可变二维视图，提供 `view[y][x]` 访问。
pub struct ScreenArrayBaseViewMut<'a, T: num_traits::Zero + Copy> {
    slice: &'a mut [T],
    width: usize,
}

impl<'a, T: num_traits::Zero + Copy> ScreenArrayBaseViewMut<'a, T> {
    impl_screen_size_methods!(width, slice);
    impl_screen_get!(width, height, slice);

    /// 带边界检查的可变像素访问。
    pub fn get_mut(&mut self, x: usize, y: usize) -> Option<&mut T> {
        if x < self.width && y < self.height() {
            Some(&mut self.slice[y * self.width + x])
        } else {
            None
        }
    }
}

impl<'a, T: num_traits::Zero + Copy> std::ops::Index<usize> for ScreenArrayBaseViewMut<'a, T> {
    type Output = [T];

    fn index(&self, row: usize) -> &Self::Output {
        let start = row
            .checked_mul(self.width)
            .expect("[ScreenArrayBaseViewMut<T>::index] overflow");
        &self.slice[start..start + self.width]
    }
}

impl<'a, T: num_traits::Zero + Copy> std::ops::IndexMut<usize> for ScreenArrayBaseViewMut<'a, T> {
    fn index_mut(&mut self, row: usize) -> &mut Self::Output {
        let start = row
            .checked_mul(self.width)
            .expect("[ScreenArrayBaseViewMut<T>::index_mut] overflow");
        &mut self.slice[start..start + self.width]
    }
}

// -------------------------------------------------------------------------------------------------
// 初始化选项枚举
// -------------------------------------------------------------------------------------------------

/// 调整大小时新内存的初始化方式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResizeInit {
    /// 不进行任何初始化（新增部分未定义，读取为 UB）。
    Uninit,
    /// 整个新缓冲区清零（使用 `T::zero()`）。
    Zero,
    /// 线性拷贝旧数据（前 min(old_len, new_len) 个元素），新增部分未初始化。
    LinearCopy,
    /// 按二维坐标左上角对齐拷贝重叠区域，新增部分未初始化。
    AlignCopy,
    /// 先清零，再线性拷贝旧数据（新增部分为 0，重叠部分被旧数据覆盖）。
    ZeroLinearCopy,
    /// 先清零，再按二维坐标对齐拷贝旧数据（新增部分为 0，重叠部分被旧数据覆盖）。
    ZeroAlignCopy,
}

// -------------------------------------------------------------------------------------------------
// ScreenArrayBase 实现
// -------------------------------------------------------------------------------------------------

impl<T: num_traits::Zero + Copy> ScreenArrayBase<T> {
    /// 从切片构造一个固定地址的堆缓冲。
    ///
    /// # Panics
    /// 如果 `data.len() != width * height`，则 panic。
    pub fn new(data: &[T], width: usize, height: usize) -> Self {
        let len = width
            .checked_mul(height)
            .expect("[ScreenArrayBase<T>::new] width * height overflow");
        assert_eq!(
            data.len(),
            len,
            "[ScreenArrayBase<T>::new] data length does not match width * height"
        );
        let ptr = alloc_buffer::<T>(len);
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), ptr, len);
        }
        Self {
            arr_ptr: ptr,
            width,
            height,
        }
    }

    /// 在堆上分配未初始化的二维缓冲。
    ///
    /// # Safety
    /// 调用者必须在使用前初始化所有内存，否则读取未定义行为。
    pub fn new_uninit(width: usize, height: usize) -> Self {
        let len = width
            .checked_mul(height)
            .expect("[ScreenArrayBase<T>::new_uninit] width * height overflow");
        let ptr = alloc_buffer::<T>(len);
        Self {
            arr_ptr: ptr,
            width,
            height,
        }
    }

    /// 在堆上分配二维缓冲并用 `T::zero()` 初始化每个元素。
    pub fn zero(width: usize, height: usize) -> Self {
        let len = width
            .checked_mul(height)
            .expect("[ScreenArrayBase<T>::zero] width * height overflow");
        let ptr = alloc_buffer::<T>(len);
        if len > 0 {
            unsafe {
                // 遍历写入零值
                for i in 0..len {
                    ptr::write(ptr.add(i), T::zero());
                }
            }
        }
        Self {
            arr_ptr: ptr,
            width,
            height,
        }
    }

    /// 从原始指针创建 `ScreenArrayBase`（指针指向连续的 `T` 数组，长度为 `width * height`）。
    ///
    /// # Safety
    /// - 指针必须指向一块有效的、大小为 `size_of::<T>() * width * height` 的堆内存
    /// - 该内存必须是由 `Box::into_raw` 或类似方式分配的
    /// - 调用者必须确保不会发生 double-free
    /// - 指针不能为 null
    pub unsafe fn from_raw(arr_ptr: *mut T, width: usize, height: usize) -> Self {
        assert!(!arr_ptr.is_null(), "[ScreenArrayBase<T>::from_raw] null pointer");
        Self {
            arr_ptr,
            width,
            height,
        }
    }

    /// 手动释放堆内存。
    ///
    /// # Safety
    /// - 只能调用一次。
    /// - 调用后不能再访问任何方法。
    pub fn drop(&mut self) {
        unsafe {
            dealloc_buffer(self.arr_ptr, self.width * self.height);
            self.arr_ptr = ptr::null_mut();
        }
    }

    /// 静态释放函数，用于脱离 `ScreenArrayBase` 对象释放内存。
    ///
    /// # Safety
    /// - `arr_ptr` 必须是由 `ScreenArrayBase` 分配且未释放过的指针。
    /// - `len` 必须等于分配时的元素个数。
    pub unsafe fn drop_it(arr_ptr: *mut T, len: usize) {
        unsafe {
            dealloc_buffer(arr_ptr, len);
        }
    }

    /// 返回当前分辨率 (高, 宽)。
    pub const fn size(&self) -> (usize, usize) {
        (self.height, self.width)
    }

    /// 返回当前宽度。
    pub const fn width(&self) -> usize {
        self.width
    }

    /// 返回当前高度。
    pub const fn height(&self) -> usize {
        self.height
    }

    /// 获取内部数据的不可变二维视图。
    pub fn get(&self) -> ScreenArrayBaseView<'_, T> {
        ScreenArrayBaseView {
            slice: self.as_slice(),
            width: self.width,
        }
    }

    /// 获取内部数据的可变二维视图。
    pub fn get_mut(&self) -> ScreenArrayBaseViewMut<'_, T> {
        ScreenArrayBaseViewMut {
            slice: self.as_mut_slice(),
            width: self.width,
        }
    }

    /// 通过坐标访问像素（可变），不检查边界。
    ///
    /// # Safety
    /// - `x < width && y < height` 必须成立。
    /// - 不得造成数据竞争。
    pub unsafe fn get_from_index_mut(&self, x: usize, y: usize) -> &mut T {
        unsafe { &mut *self.arr_ptr.add(y * self.width + x) }
    }

    /// 通过坐标访问像素（不可变），不检查边界。
    ///
    /// # Safety
    /// - `x < width && y < height` 必须成立。
    pub unsafe fn get_from_index(&self, x: usize, y: usize) -> &T {
        unsafe { &*self.arr_ptr.add(y * self.width + x) }
    }

    /// 返回指向连续缓冲区的原始指针。
    pub fn get_ptr(&self) -> *mut T {
        self.arr_ptr
    }

    /// 返回指针的数值形式（仅用于调试/FFI）。
    pub unsafe fn get_ptr_num(&self) -> usize {
        self.arr_ptr as usize
    }

    /// 返回指定像素的原始指针，不检查边界。
    ///
    /// # Safety
    /// - `x < width && y < height` 必须成立。
    pub unsafe fn get_from_index_ptr(&self, x: usize, y: usize) -> *mut T {
        unsafe { self.arr_ptr.add(y * self.width + x) }
    }

    /// 完全脱离对象的像素指针计算。
    ///
    /// # Safety
    /// - `self_ptr` 必须指向足够大的连续缓冲区（至少 `(y * width + x + 1)` 个 T）。
    /// - `x < width` 且 `y` 有效。
    pub unsafe fn get_from_index_ptr_selfless(
        self_ptr: usize,
        width: usize,
        x: usize,
        y: usize,
    ) -> *mut T {
        let ptr = self_ptr as *mut T;
        unsafe { ptr.add(y * width + x) }
    }

    /// 返回行优先、连续的一维不可变切片。
    pub fn as_slice(&self) -> &[T] {
        unsafe {
            if self.width * self.height == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(self.arr_ptr, self.width * self.height)
            }
        }
    }

    /// 返回行优先、连续的一维可变切片。
    pub fn as_mut_slice(&self) -> &mut [T] {
        unsafe {
            if self.width * self.height == 0 {
                &mut []
            } else {
                std::slice::from_raw_parts_mut(self.arr_ptr, self.width * self.height)
            }
        }
    }

    // ---------------------------------------------------------------------------------------------
    // 尺寸调整
    // ---------------------------------------------------------------------------------------------

    /// 仅更改内部宽高字段，不检查内存是否足够，不重新分配。
    ///
    /// # Safety
    /// - 调用者必须确保新的 `width * height` 不超过实际已分配的元素个数。
    /// - 更改后，所有通过宽高进行的内存访问必须在有效范围内。
    pub unsafe fn set_size_unchecked(&mut self, new_width: usize, new_height: usize) {
        self.width = new_width;
        self.height = new_height;
    }

    /// 调整尺寸并按照指定方式初始化新内存，返回旧内存指针（不释放）。
    ///
    /// # 行为
    /// - 如果新旧总长度相同，仅更新宽高，返回 `0`。
    /// - 否则分配新内存，按照 `init` 进行初始化，更新宽高，返回旧内存指针。
    ///
    /// # Returns
    /// 旧内存的指针（`usize`）。如果无需释放旧内存（例如旧长度为 0 或尺寸未变化）返回 `0`。
    ///
    /// # Safety
    /// - 调用者必须负责释放返回的旧内存（如果非零），长度应为调用前的 `width * height`。
    /// - 使用 `Uninit`、`LinearCopy` 或 `AlignCopy` 时，新增部分未初始化，调用者需在使用前初始化。
    pub unsafe fn resize_take_old(
        &mut self,
        new_width: usize,
        new_height: usize,
        init: ResizeInit,
    ) -> usize {
        let new_len = new_width
            .checked_mul(new_height)
            .expect("[ScreenArrayBase::resize_take_old] width * height overflow");
        let old_len = self.width * self.height;
        let old_ptr = self.arr_ptr;

        // 尺寸未变，直接更新元数据
        if new_len == old_len {
            self.width = new_width;
            self.height = new_height;
            return 0;
        }

        let new_ptr = alloc_buffer::<T>(new_len);

        // 根据初始化策略执行操作
        match init {
            ResizeInit::Uninit => {
                // 无需任何操作
            }
            ResizeInit::Zero => {
                unsafe { Self::zero_buf(new_ptr, new_len); }
            }
            ResizeInit::LinearCopy => {
                unsafe { Self::copy_linear(old_ptr, new_ptr, old_len.min(new_len)); }
            }
            ResizeInit::AlignCopy => {
                unsafe {
                    Self::copy_align(
                        old_ptr,
                        new_ptr,
                        self.width,
                        new_width,
                        self.height,
                        new_height,
                    );
                }
            }
            ResizeInit::ZeroLinearCopy => {
                unsafe {
                    Self::zero_buf(new_ptr, new_len);
                    Self::copy_linear(old_ptr, new_ptr, old_len.min(new_len));
                }
            }
            ResizeInit::ZeroAlignCopy => {
                unsafe {
                    Self::zero_buf(new_ptr, new_len);
                    Self::copy_align(
                        old_ptr,
                        new_ptr,
                        self.width,
                        new_width,
                        self.height,
                        new_height,
                    );
                }
            }
        }

        // 更新自身状态
        self.arr_ptr = new_ptr;
        self.width = new_width;
        self.height = new_height;

        old_ptr as usize
    }

    // --- 辅助函数 ---

    #[inline]
    unsafe fn zero_buf(ptr: *mut T, len: usize) {
        for i in 0..len {
            unsafe { ptr::write(ptr.add(i), T::zero()); }
        }
    }

    #[inline]
    unsafe fn copy_linear(src: *mut T, dst: *mut T, len: usize) {
        if len > 0 {
            unsafe { ptr::copy_nonoverlapping(src, dst, len); }
        }
    }

    #[inline]
    unsafe fn copy_align(
        src: *mut T,
        dst: *mut T,
        old_width: usize,
        new_width: usize,
        old_height: usize,
        new_height: usize,
    ) {
        let copy_height = old_height.min(new_height);
        let copy_width = old_width.min(new_width);
        for y in 0..copy_height {
            unsafe {
                Self::copy_linear(src.add(y * old_width), dst.add(y * new_width), copy_width);
            }
        }
    }

    /// 调整尺寸并按照指定方式初始化新内存，自动释放旧内存。
    ///
    /// # Safety
    /// - 使用 `Uninit`、`LinearCopy` 或 `AlignCopy` 时，新增部分未初始化，调用者需在使用前初始化。
    /// - 其他变体（`Zero*`）已完全初始化，但方法整体仍标记为 unsafe 以涵盖所有情况。
    pub unsafe fn resize(&mut self, new_width: usize, new_height: usize, init: ResizeInit) {
        unsafe {
            let old_len = self.width * self.height;
            let old_ptr = self.resize_take_old(new_width, new_height, init);
            if old_ptr != 0 {
                dealloc_buffer(old_ptr as *mut T, old_len);
            }
        }
    }

    /// 安全地调整尺寸，新内存置零，旧数据按二维坐标对齐拷贝。
    ///
    /// 等价于 `resize(new_width, new_height, ResizeInit::ZeroAlignCopy)`。
    pub fn resize_align(&mut self, new_width: usize, new_height: usize) {
        unsafe {
            self.resize(new_width, new_height, ResizeInit::ZeroAlignCopy);
        }
    }
}

// -------------------------------------------------------------------------------------------------
// 图像加载功能（需启用 feature = "array_from_image"）
// -------------------------------------------------------------------------------------------------

#[cfg(feature = "array_from_image")]
impl<T: num_traits::Zero + Copy> ScreenArrayBase<T> {
    /// 从指定路径加载图像，并使用提供的转换闭包将每个像素转换为 `T` 后写入当前实例。
    ///
    /// # 参数
    /// - `path`: 图像文件路径。
    /// - `converter`: 一个闭包，接收打包为 `u32` 的像素值（格式：0xAARRGGBB），返回 `T`。
    ///
    /// # 返回
    /// - `Ok(())` 如果图像成功加载且尺寸匹配。
    /// - `Err(Box<dyn Error>)` 如果文件打开失败、解码失败或尺寸不匹配。
    pub fn set_image<P: AsRef<Path>, F: Fn(u32) -> T>(
        &mut self,
        path: P,
        converter: F,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let img = image::open(path)?;
        let (w, h) = img.dimensions();
        let w = w as usize;
        let h = h as usize;
        if w != self.width || h != self.height {
            return Err("[ScreenArrayBase<T>::set_image] image dimensions do not match".into());
        }
        let raw = img.into_rgba8().into_raw();
        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y * self.width + x;
                let r = raw[idx * 4] as u32;
                let g = raw[idx * 4 + 1] as u32;
                let b = raw[idx * 4 + 2] as u32;
                let a = raw[idx * 4 + 3] as u32;
                let packed = a << 24 | r << 16 | g << 8 | b;
                let pixel = converter(packed);
                unsafe {
                    *self.get_from_index_mut(x, y) = pixel;
                }
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
    /// - `Ok(ScreenArrayBase<T>)` 如果图像加载成功且尺寸匹配。
    /// - `Err(Box<dyn Error>)` 如果文件打开、解码失败或尺寸不匹配。
    pub fn from_image<P: AsRef<Path> + Clone, F: Fn(u32) -> T>(
        path: P,
        converter: F,
    ) -> Result<ScreenArrayBase<T>, Box<dyn std::error::Error>> {
        let img = image::open(path.clone())?;
        let (w, h) = img.dimensions();
        let mut arr = ScreenArrayBase::zero(w as usize, h as usize);
        arr.set_image(path, converter)?;
        Ok(arr)
    }
}

// 便捷转换：当 T 实现了 From<u32> 时，可以使用 TryFrom<DynamicImage> 自动转换
#[cfg(feature = "array_from_image")]
impl<T: num_traits::Zero + Copy + From<u32>> TryFrom<image::DynamicImage> for ScreenArrayBase<T> {
    type Error = Box<dyn std::error::Error>;

    fn try_from(value: image::DynamicImage) -> Result<Self, Self::Error> {
        let (w, h) = value.dimensions();
        let raw = value.into_rgba8().into_raw();
        let pixels: Vec<T> = try_from_image_inner(raw);
        Ok(ScreenArrayBase::new(&pixels, w as usize, h as usize))
    }
}

// -------------------------------------------------------------------------------------------------
// Default 实现
// -------------------------------------------------------------------------------------------------

impl<T: num_traits::Zero + Copy> Default for ScreenArrayBase<T> {
    fn default() -> Self {
        Self::zero(0, 0)
    }
}

// -------------------------------------------------------------------------------------------------
// 线程安全
// -------------------------------------------------------------------------------------------------

// 允许跨线程传递原始指针（需外部同步访问内容）
unsafe impl<T: num_traits::Zero + Copy + Send> Send for ScreenArrayBase<T> {}
unsafe impl<T: num_traits::Zero + Copy + Sync> Sync for ScreenArrayBase<T> {}

// -------------------------------------------------------------------------------------------------
// 测试
// -------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::ops::Add;
    use super::*;

    // 辅助函数：创建一个已初始化的 ScreenArrayBase<u32>
    fn create_test_array() -> ScreenArrayBase<u32> {
        let data = vec![1, 2, 3, 4, 5, 6]; // 2x3
        ScreenArrayBase::new(&data, 3, 2)
    }

    #[test]
    fn test_new_and_index() {
        let mut arr = create_test_array();
        assert_eq!(arr.width(), 3);
        assert_eq!(arr.height(), 2);

        let view = arr.get();
        assert_eq!(view[0][0], 1);
        assert_eq!(view[0][1], 2);
        assert_eq!(view[0][2], 3);
        assert_eq!(view[1][0], 4);
        assert_eq!(view[1][1], 5);
        assert_eq!(view[1][2], 6);

        arr.drop();
    }

    #[test]
    fn test_zero_allocation() {
        let mut arr = ScreenArrayBase::<u32>::zero(3, 2);
        let slice = arr.as_slice();
        assert_eq!(slice, &[0, 0, 0, 0, 0, 0]);
        arr.drop();
    }

    #[test]
    fn test_new_uninit_and_write() {
        let mut arr = ScreenArrayBase::<u32>::new_uninit(4, 1);
        let slice = arr.as_mut_slice();
        for (i, val) in slice.iter_mut().enumerate() {
            *val = i as u32;
        }
        assert_eq!(arr.as_slice(), &[0, 1, 2, 3]);
        arr.drop();
    }

    #[test]
    fn test_get_mut() {
        let mut arr = create_test_array();
        {
            let mut view = arr.get_mut();
            view[0][0] = 100;
            assert_eq!(view[0][0], 100);
        }
        assert_eq!(arr.as_slice()[0], 100);
        arr.drop();
    }

    #[test]
    #[should_panic]
    fn test_view_index_out_of_bounds_panics() {
        let mut arr = create_test_array();
        let view = arr.get();
        let _ = view[2][0]; // 行越界
        arr.drop(); // 不会执行到，因为 panic
    }

    #[test]
    #[should_panic]
    fn test_view_index_row_overflow_panics() {
        let mut arr = ScreenArrayBase::<u32>::zero(usize::MAX, 1); // 分配会失败，这里仅测试索引逻辑，实际不会执行
        let view = arr.get();
        let _ = view[usize::MAX];
        arr.drop();
    }

    #[test]
    fn test_get_from_index_unsafe() {
        let mut arr = create_test_array();
        unsafe {
            assert_eq!(*arr.get_from_index(0, 0), 1);
            assert_eq!(*arr.get_from_index_mut(1, 1), 5);
        }
        arr.drop();
    }

    #[test]
    fn test_resize_align() {
        let mut arr = ScreenArrayBase::<u32>::new(&[1, 2, 3, 4, 5, 6], 3, 2);
        unsafe {
            arr.resize(4, 3, ResizeInit::ZeroAlignCopy);
        }
        let view = arr.get();
        assert_eq!(view[0], [1, 2, 3, 0]);
        assert_eq!(view[1], [4, 5, 6, 0]);
        assert_eq!(view[2], [0, 0, 0, 0]);
        arr.drop();
    }

    #[test]
    fn test_resize_linear_copy() {
        let mut arr = ScreenArrayBase::<u32>::new(&[1, 2, 3, 4], 2, 2);
        unsafe {
            arr.resize(3, 2, ResizeInit::LinearCopy);
        }
        // 只检查已复制的部分，新增部分未初始化
        let view = arr.get();
        assert_eq!(view[0][..3], [1, 2, 3]);
        assert_eq!(view[1][0], 4);
        arr.drop();
    }

    #[test]
    fn test_resize_get_old() {
        let mut arr = ScreenArrayBase::<u32>::new(&[1, 2, 3, 4], 2, 2);
        let old_len = 4;
        let old_ptr = unsafe { arr.resize_take_old(3, 3, ResizeInit::Zero) };
        assert_ne!(old_ptr, 0);
        assert_eq!(arr.width(), 3);
        assert_eq!(arr.height(), 3);
        unsafe {
            let old_slice = std::slice::from_raw_parts(old_ptr as *mut u32, old_len);
            assert_eq!(old_slice, &[1, 2, 3, 4]);
            ScreenArrayBase::<u32>::drop_it(old_ptr as *mut u32, old_len);
        }
        arr.drop();
    }

    #[test]
    fn test_resize_same_size_returns_zero() {
        let mut arr = ScreenArrayBase::<u32>::new(&[1, 2, 3, 4], 2, 2);
        let old_ptr = unsafe { arr.resize_take_old(4, 1, ResizeInit::Zero) };
        assert_eq!(old_ptr, 0);
        assert_eq!(arr.width(), 4);
        assert_eq!(arr.height(), 1);
        arr.drop();
    }

    #[test]
    fn test_set_size_unchecked() {
        let mut arr = ScreenArrayBase::<u32>::new(&[1, 2, 3, 4, 5, 6], 3, 2);
        unsafe {
            arr.set_size_unchecked(2, 3);
        }
        let view = arr.get();
        assert_eq!(view[0], [1, 2]);
        assert_eq!(view[1], [3, 4]);
        assert_eq!(view[2], [5, 6]);
        arr.drop();
    }

    #[test]
    fn test_as_slice_empty() {
        let mut arr = ScreenArrayBase::<u32>::zero(0, 0);
        assert!(arr.as_slice().is_empty());
        arr.drop();
    }

    #[test]
    fn test_drop_it_static() {
        let arr = ScreenArrayBase::<u32>::new(&[42; 4], 2, 2);
        let ptr = arr.get_ptr();
        let len = 4;
        std::mem::forget(arr);
        unsafe {
            ScreenArrayBase::<u32>::drop_it(ptr, len);
        }
    }

    // 自定义像素类型测试
    #[derive(Clone, Copy, Debug, PartialEq)]
    #[repr(transparent)]
    struct CustomPixel(u16);

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
    fn test_generic_custom_type() {
        let mut arr = ScreenArrayBase::<CustomPixel>::zero(2, 2);
        assert_eq!(arr.as_slice(), &[CustomPixel(0); 4]);
        arr.drop();

        let data = [
            CustomPixel(1),
            CustomPixel(2),
            CustomPixel(3),
            CustomPixel(4),
        ];
        let mut arr2 = ScreenArrayBase::<CustomPixel>::new(&data, 2, 2);
        let view = arr2.get();
        assert_eq!(view[0][1], CustomPixel(2));
        assert_eq!(view[1][0], CustomPixel(3));
        arr2.drop();
    }
}