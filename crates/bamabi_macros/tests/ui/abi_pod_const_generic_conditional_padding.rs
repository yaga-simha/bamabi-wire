use bamabi_macros::AbiPod;

#[repr(C)]
#[derive(Copy, Clone, AbiPod)]
pub struct ConditionalPadding<const N: usize> {
    pub bytes: [u8; N],
    pub value: u32,
}

fn main() {}
