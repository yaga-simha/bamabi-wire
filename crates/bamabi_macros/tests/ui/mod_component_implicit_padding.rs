use bamabi_macros::ModComponent;

#[repr(C)]
#[derive(Copy, Clone, ModComponent)]
struct PaddedModComponent {
    a: u8,
    b: u64,
}

fn main() {}
