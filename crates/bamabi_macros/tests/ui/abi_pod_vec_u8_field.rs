use bamabi_macros::AbiPod;

#[repr(C)]
#[derive(AbiPod)]
pub struct VecInPod {
    pub payload: Vec<u8>,
}

fn main() {}
