use crate::base::immutable::ScreenArrayBase;


pub mod immutable;
pub mod mutable;

pub type ScreenArray<const W: usize, const H: usize> = ScreenArrayBase<u32, W, H>;


