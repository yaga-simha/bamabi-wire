use bamabi_macros::AbiPod;

#[repr(C)]
#[derive(Copy, Clone, AbiPod)]
pub struct LifetimePod<'a> {
    pub value: &'a u32,
}

fn main() {}
