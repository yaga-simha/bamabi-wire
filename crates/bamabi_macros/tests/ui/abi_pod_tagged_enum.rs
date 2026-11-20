use bamabi_macros::AbiPod;

#[repr(C, u8)]
#[derive(Copy, Clone, AbiPod)]
pub enum TaggedPodRejected {
    None = 0,
    Damage(u32) = 1,
}

fn main() {}
