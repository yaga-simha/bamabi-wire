use bamabi_macros::AbiPod;

#[repr(C)]
#[derive(Copy, Clone, AbiPod)]
pub struct PaddedBuf<const N: usize> {
    pub flag: u8,
    pub data: [u32; N],
}

fn main() {}
