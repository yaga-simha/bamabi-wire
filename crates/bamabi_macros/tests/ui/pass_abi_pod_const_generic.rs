use bamabi_macros::AbiPod;
use bamabi_sys::{AbiPod as AbiPodTrait, RawInsertable};

#[repr(C)]
#[derive(Copy, Clone, AbiPod)]
pub struct FixedBuf<const N: usize> {
    pub len: u32,
    pub data: [u32; N],
}

#[repr(C)]
#[derive(Copy, Clone, AbiPod)]
pub struct FixedBufDefault<const N: usize = 8> {
    pub len: u32,
    pub data: [u32; N],
}

#[repr(transparent)]
#[derive(Copy, Clone, AbiPod)]
pub struct ByteArray<const N: usize>(pub [u8; N]);

#[repr(C)]
#[derive(Copy, Clone, AbiPod)]
pub struct Grid<const ROWS: usize, const COLS: usize> {
    pub cells: [[f32; COLS]; ROWS],
}

fn require_pod<T: AbiPodTrait + RawInsertable>() {}

fn main() {
    require_pod::<FixedBuf<0>>();
    require_pod::<FixedBuf<1>>();
    require_pod::<FixedBuf<16>>();
    require_pod::<FixedBufDefault>();
    require_pod::<FixedBufDefault<4>>();
    require_pod::<ByteArray<0>>();
    require_pod::<ByteArray<7>>();
    require_pod::<Grid<2, 3>>();
    bamabi_sys::assert_abi_pod::<FixedBuf<8>>();
    bamabi_sys::assert_abi_pod::<Grid<4, 4>>();
}
