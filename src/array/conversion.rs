pub fn flatten<T, const W: usize, const H: usize>(arr: &[[T; W]; H]) -> &[T] {
    unsafe { std::slice::from_raw_parts(arr.as_ptr().cast(), W * H) }
}

pub fn flatten_mut<T, const W: usize, const H: usize>(arr: &mut [[T; W]; H]) -> &mut [T] {
    unsafe { std::slice::from_raw_parts_mut(*arr.as_ptr().cast(), W * H) }
}
