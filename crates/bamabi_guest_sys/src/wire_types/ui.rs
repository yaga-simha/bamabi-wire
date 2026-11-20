//! Bevy `0.20` UI, text, headless widget (`bevy_ui_widgets`), and `bevy_feathers` wire mirror
//! types matching `bamabi_bridges::ui` byte-for-byte.

use bamabi_sys::{FfiAssetHandle, FfiBool, FfiEntity};

use super::std_types::FfiColor;

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s `Val` (`"bamabi::ui::FfiVal"`, 8 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiVal {
    /// Unit discriminant (`0 = Auto`, `1 = Px`, `2 = Percent`, `3 = Vw`, `4 = Vh`,
    /// `5 = VMin`, `6 = VMax`, `7 = Em`, `8 = Rem`).
    pub unit: u8,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
    /// Scalar magnitude (ignored when `unit == Self::UNIT_AUTO`).
    pub value: f32,
}

impl_wire_abi_pod!(
    FfiVal,
    size = 8,
    align = 4,
    schema = [
        unit: u8 => "u8",
        reserved0: [u8; 3] => "[u8;3]",
        value: f32 => "f32",
    ]
);

impl FfiVal {
    /// `Val::Auto` (`0`).
    pub const UNIT_AUTO: u8 = 0;
    /// `Val::Px` (`1`).
    pub const UNIT_PX: u8 = 1;
    /// `Val::Percent` (`2`).
    pub const UNIT_PERCENT: u8 = 2;
    /// `Val::Vw` (`3`).
    pub const UNIT_VW: u8 = 3;
    /// `Val::Vh` (`4`).
    pub const UNIT_VH: u8 = 4;
    /// `Val::VMin` (`5`).
    pub const UNIT_VMIN: u8 = 5;
    /// `Val::VMax` (`6`).
    pub const UNIT_VMAX: u8 = 6;
    /// `Val::Em` (`7`, Bevy `0.20`).
    pub const UNIT_EM: u8 = 7;
    /// `Val::Rem` (`8`, Bevy `0.20`).
    pub const UNIT_REM: u8 = 8;

    /// `Val::Auto` constant.
    pub const AUTO: Self = Self {
        unit: Self::UNIT_AUTO,
        reserved0: [0; 3],
        value: 0.0,
    };

    /// `Val::Px(0.0)` constant.
    pub const ZERO: Self = Self::px(0.0);

    /// Constructs `Val::Px(v)`.
    #[inline]
    pub const fn px(value: f32) -> Self {
        Self {
            unit: Self::UNIT_PX,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `Val::Percent(v)`.
    #[inline]
    pub const fn percent(value: f32) -> Self {
        Self {
            unit: Self::UNIT_PERCENT,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `Val::Vw(v)`.
    #[inline]
    pub const fn vw(value: f32) -> Self {
        Self {
            unit: Self::UNIT_VW,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `Val::Vh(v)`.
    #[inline]
    pub const fn vh(value: f32) -> Self {
        Self {
            unit: Self::UNIT_VH,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `Val::VMin(v)`.
    #[inline]
    pub const fn vmin(value: f32) -> Self {
        Self {
            unit: Self::UNIT_VMIN,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `Val::VMax(v)`.
    #[inline]
    pub const fn vmax(value: f32) -> Self {
        Self {
            unit: Self::UNIT_VMAX,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `Val::Em(v)` (relative to node `EmSize`).
    #[inline]
    pub const fn em(value: f32) -> Self {
        Self {
            unit: Self::UNIT_EM,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `Val::Rem(v)` (relative to root `RemSize`).
    #[inline]
    pub const fn rem(value: f32) -> Self {
        Self {
            unit: Self::UNIT_REM,
            reserved0: [0; 3],
            value,
        }
    }
}

impl Default for FfiVal {
    #[inline]
    fn default() -> Self {
        Self::AUTO
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `UiRect` (`"bamabi::ui::FfiUiRect"`, 32 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiUiRect {
    /// Left edge inset.
    pub left: FfiVal,
    /// Right edge inset.
    pub right: FfiVal,
    /// Top edge inset.
    pub top: FfiVal,
    /// Bottom edge inset.
    pub bottom: FfiVal,
}

impl_wire_abi_pod!(
    FfiUiRect,
    size = 32,
    align = 4,
    schema = [
        left: FfiVal => "FfiVal",
        right: FfiVal => "FfiVal",
        top: FfiVal => "FfiVal",
        bottom: FfiVal => "FfiVal",
    ]
);

impl FfiUiRect {
    /// All edges `Val::Px(0.0)`.
    pub const ZERO: Self = Self::all(FfiVal::ZERO);

    /// Sets all four edges to `v`.
    #[inline]
    pub const fn all(v: FfiVal) -> Self {
        Self {
            left: v,
            right: v,
            top: v,
            bottom: v,
        }
    }

    /// Sets left/right to `horizontal` and top/bottom to `vertical`.
    #[inline]
    pub const fn axes(horizontal: FfiVal, vertical: FfiVal) -> Self {
        Self {
            left: horizontal,
            right: horizontal,
            top: vertical,
            bottom: vertical,
        }
    }

    /// Sets all four edges to `Val::Px(v)`.
    #[inline]
    pub const fn px(v: f32) -> Self {
        Self::all(FfiVal::px(v))
    }

    /// Constructs a [`FfiUiRect`] from explicit `(left, right, top, bottom)` edge values.
    #[inline]
    pub const fn new(left: FfiVal, right: FfiVal, top: FfiVal, bottom: FfiVal) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }
}

impl Default for FfiUiRect {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s elliptical `CornerRadius`
/// (`"bamabi::ui::FfiCornerRadius"`, 16 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiCornerRadius {
    /// Horizontal corner radius (`CornerRadius::x`).
    pub x: FfiVal,
    /// Vertical corner radius (`CornerRadius::y`; [`FfiVal::AUTO`] produces a circular corner).
    pub y: FfiVal,
}

impl_wire_abi_pod!(
    FfiCornerRadius,
    size = 16,
    align = 4,
    schema = [
        x: FfiVal => "FfiVal",
        y: FfiVal => "FfiVal",
    ]
);

impl FfiCornerRadius {
    /// Zero circular corner radius (`x = 0px, y = Auto`).
    pub const ZERO: Self = Self {
        x: FfiVal::ZERO,
        y: FfiVal::AUTO,
    };

    /// Circular corner radius (`x = radius, y = FfiVal::AUTO`, matching Bevy `0.20`'s `CornerRadius`).
    #[inline]
    pub const fn circular(radius: FfiVal) -> Self {
        Self {
            x: radius,
            y: FfiVal::AUTO,
        }
    }

    /// Elliptical corner radius `(x, y)`.
    #[inline]
    pub const fn elliptical(x: FfiVal, y: FfiVal) -> Self {
        Self { x, y }
    }

    /// Circular corner radius in logical pixels.
    #[inline]
    pub const fn px(value: f32) -> Self {
        Self::circular(FfiVal::px(value))
    }
}

impl Default for FfiCornerRadius {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s `BorderRadius`
/// (`"bamabi::ui::FfiBorderRadius"`, 64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiBorderRadius {
    /// Top-left elliptical corner radius.
    pub top_left: FfiCornerRadius,
    /// Top-right elliptical corner radius.
    pub top_right: FfiCornerRadius,
    /// Bottom-right elliptical corner radius.
    pub bottom_right: FfiCornerRadius,
    /// Bottom-left elliptical corner radius.
    pub bottom_left: FfiCornerRadius,
}

impl_wire_abi_pod!(
    FfiBorderRadius,
    size = 64,
    align = 4,
    schema = [
        top_left: FfiCornerRadius => "FfiCornerRadius",
        top_right: FfiCornerRadius => "FfiCornerRadius",
        bottom_right: FfiCornerRadius => "FfiCornerRadius",
        bottom_left: FfiCornerRadius => "FfiCornerRadius",
    ]
);

impl FfiBorderRadius {
    /// Zero border radius on all four corners.
    pub const ZERO: Self = Self::all(FfiCornerRadius::ZERO);

    /// Uniform corner radius across all four corners.
    #[inline]
    pub const fn all(corner: FfiCornerRadius) -> Self {
        Self {
            top_left: corner,
            top_right: corner,
            bottom_right: corner,
            bottom_left: corner,
        }
    }

    /// Uniform circular pixel radius on all four corners.
    #[inline]
    pub const fn px(radius: f32) -> Self {
        Self::all(FfiCornerRadius::px(radius))
    }

    /// Sets all four corners to the same elliptical `(x, y)` radius.
    #[inline]
    pub const fn elliptical_all(x: FfiVal, y: FfiVal) -> Self {
        Self::all(FfiCornerRadius::elliptical(x, y))
    }

    /// Sets each corner to a distinct circular logical-pixel radius
    /// `(top_left, top_right, bottom_right, bottom_left)`.
    #[inline]
    pub const fn corners_px(
        top_left: f32,
        top_right: f32,
        bottom_right: f32,
        bottom_left: f32,
    ) -> Self {
        Self {
            top_left: FfiCornerRadius::px(top_left),
            top_right: FfiCornerRadius::px(top_right),
            bottom_right: FfiCornerRadius::px(bottom_right),
            bottom_left: FfiCornerRadius::px(bottom_left),
        }
    }
}

impl Default for FfiBorderRadius {
    #[inline]
    fn default() -> Self {
        Self::ZERO
    }
}

/// Zero-padding `#[repr(C)]` sRGBA color wire type used across `bamabi_bridges::ui`
/// (`"bamabi::ui::FfiRgba"`, 16 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiRgba {
    /// Red channel in sRGB space (`0.0..=1.0`).
    pub r: f32,
    /// Green channel in sRGB space (`0.0..=1.0`).
    pub g: f32,
    /// Blue channel in sRGB space (`0.0..=1.0`).
    pub b: f32,
    /// Alpha channel (`0.0..=1.0`).
    pub a: f32,
}

impl_wire_abi_pod!(
    FfiRgba,
    size = 16,
    align = 4,
    schema = [
        r: f32 => "f32",
        g: f32 => "f32",
        b: f32 => "f32",
        a: f32 => "f32",
    ]
);

impl FfiRgba {
    /// Fully transparent black `(0, 0, 0, 0)`.
    pub const TRANSPARENT: Self = Self::new(0.0, 0.0, 0.0, 0.0);
    /// Opaque white `(1, 1, 1, 1)`.
    pub const WHITE: Self = Self::new(1.0, 1.0, 1.0, 1.0);
    /// Opaque black `(0, 0, 0, 1)`.
    pub const BLACK: Self = Self::new(0.0, 0.0, 0.0, 1.0);

    /// Constructs an sRGBA color from floating-point channels.
    #[inline]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Constructs an opaque sRGBA color from 8-bit `(r, g, b)` channels.
    #[inline]
    pub const fn srgb_u8(r: u8, g: u8, b: u8) -> Self {
        Self {
            r: (r as f32) / 255.0,
            g: (g as f32) / 255.0,
            b: (b as f32) / 255.0,
            a: 1.0,
        }
    }

    /// Returns a copy with alpha replaced by `a`.
    #[inline]
    #[must_use]
    pub const fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        self
    }

    /// Returns `[r, g, b, a]`.
    #[inline]
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

impl Default for FfiRgba {
    #[inline]
    fn default() -> Self {
        Self::WHITE
    }
}

impl From<FfiColor> for FfiRgba {
    #[inline]
    fn from(c: FfiColor) -> Self {
        Self::new(c.r, c.g, c.b, c.a)
    }
}

impl From<FfiRgba> for FfiColor {
    #[inline]
    fn from(c: FfiRgba) -> Self {
        Self::srgba(c.r, c.g, c.b, c.a)
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s 4-edge `BorderColor`
/// (`"bamabi::ui::FfiBorderColor"`, 64 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiBorderColor {
    /// Top border color.
    pub top: FfiRgba,
    /// Right border color.
    pub right: FfiRgba,
    /// Bottom border color.
    pub bottom: FfiRgba,
    /// Left border color.
    pub left: FfiRgba,
}

impl_wire_abi_pod!(
    FfiBorderColor,
    size = 64,
    align = 4,
    schema = [
        top: FfiRgba => "FfiRgba",
        right: FfiRgba => "FfiRgba",
        bottom: FfiRgba => "FfiRgba",
        left: FfiRgba => "FfiRgba",
    ]
);

impl FfiBorderColor {
    /// Sets all four border edges to `color`.
    #[inline]
    pub const fn all(color: FfiRgba) -> Self {
        Self {
            top: color,
            right: color,
            bottom: color,
            left: color,
        }
    }
}

impl Default for FfiBorderColor {
    #[inline]
    fn default() -> Self {
        Self::all(FfiRgba::TRANSPARENT)
    }
}

/// Fixed-layout `#[repr(transparent)]` wire projection of Bevy `0.20`'s `RemSize` root font-size
/// resource (in logical pixels, default `20.0` matching `bevy_text::DEFAULT_REM_SIZE_PX`).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiRemSize(pub f32);

impl_wire_abi_pod!(FfiRemSize, size = 4, align = 4, fields = [f32]);

impl Default for FfiRemSize {
    #[inline]
    fn default() -> Self {
        Self(20.0)
    }
}

/// Fixed-layout `#[repr(transparent)]` wire projection of Bevy `0.20`'s `EmSize` per-node
/// font-size component (in logical pixels, default `20.0` matching `bevy_text::DEFAULT_REM_SIZE_PX`).
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiEmSize(pub f32);

impl_wire_abi_pod!(FfiEmSize, size = 4, align = 4, fields = [f32]);

impl Default for FfiEmSize {
    #[inline]
    fn default() -> Self {
        Self(20.0)
    }
}

/// Zero-padding `#[repr(C)]` layout descriptor mirroring Bevy `0.20`'s `Node` + `FixedNode`
/// (`"bamabi::ui::FfiUiNode"`, 280 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiUiNode {
    /// Preferred node width (`Node::width`).
    pub width: FfiVal,
    /// Preferred node height (`Node::height`).
    pub height: FfiVal,
    /// Minimum node width (`Node::min_width`).
    pub min_width: FfiVal,
    /// Minimum node height (`Node::min_height`).
    pub min_height: FfiVal,
    /// Maximum node width (`Node::max_width`).
    pub max_width: FfiVal,
    /// Maximum node height (`Node::max_height`).
    pub max_height: FfiVal,
    /// Left offset (`Node::left`).
    pub left: FfiVal,
    /// Right offset (`Node::right`).
    pub right: FfiVal,
    /// Top offset (`Node::top`).
    pub top: FfiVal,
    /// Bottom offset (`Node::bottom`).
    pub bottom: FfiVal,
    /// Flex basis (`Node::flex_basis`).
    pub flex_basis: FfiVal,
    /// Gutter between rows (`Node::row_gap`).
    pub row_gap: FfiVal,
    /// Gutter between columns (`Node::column_gap`).
    pub column_gap: FfiVal,
    /// Inner padding (`Node::padding`).
    pub padding: FfiUiRect,
    /// Outer margin (`Node::margin`).
    pub margin: FfiUiRect,
    /// Border thickness (`Node::border`).
    pub border: FfiUiRect,
    /// Elliptical corner radii (`Node::border_radius`).
    pub border_radius: FfiBorderRadius,
    /// Flex grow factor (`Node::flex_grow`).
    pub flex_grow: f32,
    /// Flex shrink factor (`Node::flex_shrink`, default `1.0`).
    pub flex_shrink: f32,
    /// Layout algorithm (`0 = Flex`, `1 = Grid`, `2 = Block`, `3 = None`).
    pub display: u8,
    /// Positioning mode (`0 = Relative`, `1 = Absolute`).
    pub position_type: u8,
    /// Flexbox main axis (`0 = Row`, `1 = Column`, `2 = RowReverse`, `3 = ColumnReverse`).
    pub flex_direction: u8,
    /// Flexbox wrapping (`0 = NoWrap`, `1 = Wrap`, `2 = WrapReverse`).
    pub flex_wrap: u8,
    /// Cross-axis alignment (`0 = Default`, `1 = Start`, ..., `12 = Stretch`).
    pub align_items: u8,
    /// Main-axis justification (`0 = Default`, `1 = Start`, ..., `14 = SpaceAround`).
    pub justify_content: u8,
    /// When non-zero, attaches Bevy `0.20`'s `FixedNode` marker so this node is laid out as a
    /// viewport-pinned root even when parented in a UI hierarchy.
    pub fixed_node: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiUiNode,
    size = 280,
    align = 4,
    schema = [
        width: FfiVal => "FfiVal",
        height: FfiVal => "FfiVal",
        min_width: FfiVal => "FfiVal",
        min_height: FfiVal => "FfiVal",
        max_width: FfiVal => "FfiVal",
        max_height: FfiVal => "FfiVal",
        left: FfiVal => "FfiVal",
        right: FfiVal => "FfiVal",
        top: FfiVal => "FfiVal",
        bottom: FfiVal => "FfiVal",
        flex_basis: FfiVal => "FfiVal",
        row_gap: FfiVal => "FfiVal",
        column_gap: FfiVal => "FfiVal",
        padding: FfiUiRect => "FfiUiRect",
        margin: FfiUiRect => "FfiUiRect",
        border: FfiUiRect => "FfiUiRect",
        border_radius: FfiBorderRadius => "FfiBorderRadius",
        flex_grow: f32 => "f32",
        flex_shrink: f32 => "f32",
        display: u8 => "u8",
        position_type: u8 => "u8",
        flex_direction: u8 => "u8",
        flex_wrap: u8 => "u8",
        align_items: u8 => "u8",
        justify_content: u8 => "u8",
        fixed_node: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

#[allow(missing_docs)]
impl FfiUiNode {
    pub const DISPLAY_FLEX: u8 = 0;
    pub const DISPLAY_GRID: u8 = 1;
    pub const DISPLAY_BLOCK: u8 = 2;
    pub const DISPLAY_NONE: u8 = 3;

    pub const POSITION_RELATIVE: u8 = 0;
    pub const POSITION_ABSOLUTE: u8 = 1;

    pub const FLEX_ROW: u8 = 0;
    pub const FLEX_COLUMN: u8 = 1;
    pub const FLEX_ROW_REVERSE: u8 = 2;
    pub const FLEX_COLUMN_REVERSE: u8 = 3;

    pub const WRAP_NO_WRAP: u8 = 0;
    pub const WRAP_WRAP: u8 = 1;
    pub const WRAP_WRAP_REVERSE: u8 = 2;

    pub const ALIGN_DEFAULT: u8 = 0;
    pub const ALIGN_START: u8 = 1;
    pub const ALIGN_START_SAFE: u8 = 2;
    pub const ALIGN_END: u8 = 3;
    pub const ALIGN_END_SAFE: u8 = 4;
    pub const ALIGN_FLEX_START: u8 = 5;
    pub const ALIGN_FLEX_START_SAFE: u8 = 6;
    pub const ALIGN_FLEX_END: u8 = 7;
    pub const ALIGN_FLEX_END_SAFE: u8 = 8;
    pub const ALIGN_CENTER: u8 = 9;
    pub const ALIGN_CENTER_SAFE: u8 = 10;
    pub const ALIGN_BASELINE: u8 = 11;
    pub const ALIGN_STRETCH: u8 = 12;

    pub const JUSTIFY_DEFAULT: u8 = 0;
    pub const JUSTIFY_START: u8 = 1;
    pub const JUSTIFY_START_SAFE: u8 = 2;
    pub const JUSTIFY_END: u8 = 3;
    pub const JUSTIFY_END_SAFE: u8 = 4;
    pub const JUSTIFY_FLEX_START: u8 = 5;
    pub const JUSTIFY_FLEX_START_SAFE: u8 = 6;
    pub const JUSTIFY_FLEX_END: u8 = 7;
    pub const JUSTIFY_FLEX_END_SAFE: u8 = 8;
    pub const JUSTIFY_CENTER: u8 = 9;
    pub const JUSTIFY_CENTER_SAFE: u8 = 10;
    pub const JUSTIFY_STRETCH: u8 = 11;
    pub const JUSTIFY_SPACE_BETWEEN: u8 = 12;
    pub const JUSTIFY_SPACE_EVENLY: u8 = 13;
    pub const JUSTIFY_SPACE_AROUND: u8 = 14;
}

impl Default for FfiUiNode {
    #[inline]
    fn default() -> Self {
        Self {
            width: FfiVal::AUTO,
            height: FfiVal::AUTO,
            min_width: FfiVal::AUTO,
            min_height: FfiVal::AUTO,
            max_width: FfiVal::AUTO,
            max_height: FfiVal::AUTO,
            left: FfiVal::AUTO,
            right: FfiVal::AUTO,
            top: FfiVal::AUTO,
            bottom: FfiVal::AUTO,
            flex_basis: FfiVal::AUTO,
            row_gap: FfiVal::ZERO,
            column_gap: FfiVal::ZERO,
            padding: FfiUiRect::ZERO,
            margin: FfiUiRect::ZERO,
            border: FfiUiRect::ZERO,
            border_radius: FfiBorderRadius::ZERO,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            display: Self::DISPLAY_FLEX,
            position_type: Self::POSITION_RELATIVE,
            flex_direction: Self::FLEX_ROW,
            flex_wrap: Self::WRAP_NO_WRAP,
            align_items: Self::ALIGN_DEFAULT,
            justify_content: Self::JUSTIFY_DEFAULT,
            fixed_node: FfiBool::FALSE,
            reserved0: 0,
        }
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s `FontSize` (`"bamabi::ui::FfiFontSize"`,
/// 8 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiFontSize {
    /// Unit discriminant (`0 = Px`, `1 = Vw`, `2 = Vh`, `3 = VMin`, `4 = VMax`, `5 = Rem`).
    pub unit: u8,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
    /// Scalar font-size magnitude.
    pub value: f32,
}

impl_wire_abi_pod!(
    FfiFontSize,
    size = 8,
    align = 4,
    schema = [
        unit: u8 => "u8",
        reserved0: [u8; 3] => "[u8;3]",
        value: f32 => "f32",
    ]
);

impl FfiFontSize {
    /// `FontSize::Px` (`0`).
    pub const UNIT_PX: u8 = 0;
    /// `FontSize::Vw` (`1`).
    pub const UNIT_VW: u8 = 1;
    /// `FontSize::Vh` (`2`).
    pub const UNIT_VH: u8 = 2;
    /// `FontSize::VMin` (`3`).
    pub const UNIT_VMIN: u8 = 3;
    /// `FontSize::VMax` (`4`).
    pub const UNIT_VMAX: u8 = 4;
    /// `FontSize::Rem` (`5`).
    pub const UNIT_REM: u8 = 5;

    /// Constructs `FontSize::Px(v)`.
    #[inline]
    pub const fn px(value: f32) -> Self {
        Self {
            unit: Self::UNIT_PX,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `FontSize::Vw(v)`.
    #[inline]
    pub const fn vw(value: f32) -> Self {
        Self {
            unit: Self::UNIT_VW,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `FontSize::Vh(v)`.
    #[inline]
    pub const fn vh(value: f32) -> Self {
        Self {
            unit: Self::UNIT_VH,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `FontSize::VMin(v)`.
    #[inline]
    pub const fn vmin(value: f32) -> Self {
        Self {
            unit: Self::UNIT_VMIN,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `FontSize::VMax(v)`.
    #[inline]
    pub const fn vmax(value: f32) -> Self {
        Self {
            unit: Self::UNIT_VMAX,
            reserved0: [0; 3],
            value,
        }
    }

    /// Constructs `FontSize::Rem(v)`.
    #[inline]
    pub const fn rem(value: f32) -> Self {
        Self {
            unit: Self::UNIT_REM,
            reserved0: [0; 3],
            value,
        }
    }
}

impl Default for FfiFontSize {
    #[inline]
    fn default() -> Self {
        Self::px(20.0)
    }
}

/// Zero-padding `#[repr(C)]` text styling descriptor synchronizing `TextFont`, `TextColor`,
/// `TextLayout`, and optional `Text` (`"bamabi::ui::FfiUiTextStyle"`, 64 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiUiTextStyle {
    /// Optional `Font` asset handle (`FfiAssetHandle::NONE` preserves the current font source).
    pub font_handle: FfiAssetHandle,
    /// Foreground text color (`TextColor`).
    pub color: FfiRgba,
    /// Font size (`TextFont::font_size`), supporting `FontSize::Rem` and viewport units.
    pub font_size: FfiFontSize,
    /// Variable font weight (`1..=1000`, default `400` = `FontWeight::NORMAL`).
    pub weight: u16,
    /// Horizontal justification (`0 = Left`, `1 = Center`, `2 = Right`, `3 = Justified`,
    /// `4 = Start`, `5 = End`).
    pub justify: u8,
    /// Line breaking strategy (`0 = WordBoundary`, `1 = AnyCharacter`, `2 = WordOrCharacter`,
    /// `3 = NoWrap`).
    pub linebreak: u8,
    /// Font smoothing (`0 = AntiAliased`, `1 = None`).
    pub font_smoothing: u8,
    /// Font slant style (`0 = Normal`, `1 = Italic`, `2 = Oblique`).
    pub font_style: u8,
    /// When non-zero, ensures a `Text` component exists on the entity if absent.
    pub ensure_text: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiUiTextStyle,
    size = 64,
    align = 4,
    schema = [
        font_handle: FfiAssetHandle => "FfiAssetHandle",
        color: FfiRgba => "FfiRgba",
        font_size: FfiFontSize => "FfiFontSize",
        weight: u16 => "u16",
        justify: u8 => "u8",
        linebreak: u8 => "u8",
        font_smoothing: u8 => "u8",
        font_style: u8 => "u8",
        ensure_text: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

#[allow(missing_docs)]
impl FfiUiTextStyle {
    pub const JUSTIFY_LEFT: u8 = 0;
    pub const JUSTIFY_CENTER: u8 = 1;
    pub const JUSTIFY_RIGHT: u8 = 2;
    pub const JUSTIFY_JUSTIFIED: u8 = 3;
    pub const JUSTIFY_START: u8 = 4;
    pub const JUSTIFY_END: u8 = 5;

    pub const LINEBREAK_WORD_BOUNDARY: u8 = 0;
    pub const LINEBREAK_ANY_CHARACTER: u8 = 1;
    pub const LINEBREAK_WORD_OR_CHARACTER: u8 = 2;
    pub const LINEBREAK_NO_WRAP: u8 = 3;

    pub const SMOOTHING_ANTI_ALIASED: u8 = 0;
    pub const SMOOTHING_NONE: u8 = 1;

    pub const STYLE_NORMAL: u8 = 0;
    pub const STYLE_ITALIC: u8 = 1;
    pub const STYLE_OBLIQUE: u8 = 2;
}

impl Default for FfiUiTextStyle {
    #[inline]
    fn default() -> Self {
        Self {
            font_handle: FfiAssetHandle::NONE,
            color: FfiRgba::WHITE,
            font_size: FfiFontSize::rem(1.0),
            weight: 400,
            justify: Self::JUSTIFY_LEFT,
            linebreak: Self::LINEBREAK_WORD_BOUNDARY,
            font_smoothing: Self::SMOOTHING_ANTI_ALIASED,
            font_style: Self::STYLE_NORMAL,
            ensure_text: FfiBool::FALSE,
            reserved0: 0,
        }
    }
}

/// Zero-padding `#[repr(C)]` configuration mirror for Bevy `0.20`'s `EditableText` / `TextInput`
/// (`"bamabi::ui::FfiEditableTextConfig"`, 36 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiEditableTextConfig {
    /// Cursor reveal margins as fractions of viewport size (`0.0..=0.5`).
    pub cursor_margin: [f32; 2],
    /// Cursor width relative to font size (default `0.2`).
    pub cursor_width: f32,
    /// Cursor blink period in seconds (default `1.0`).
    pub cursor_blink_period_secs: f32,
    /// Height in number of visible lines when `has_visible_lines` is set.
    pub visible_lines: f32,
    /// Width in number of visible glyphs when `has_visible_width` is set.
    pub visible_width: f32,
    /// Maximum character count when `has_max_characters` is set.
    pub max_characters: u32,
    /// When non-zero, attaches `bevy_ui_widgets::TextInput`; when zero, keeps pure headless
    /// `bevy_text::EditableText` without `TextInput`.
    pub interactive: FfiBool,
    /// Whether multi-line input (`\n`) is permitted (`EditableText::allow_newlines`).
    pub allow_newlines: FfiBool,
    /// `TextReadWriteMode` discriminant (`0 = Editable`, `1 = ReadOnly`, `2 = Static`).
    pub read_write_mode: u8,
    /// Whether `max_characters` is engaged (`0 = None`, `1 = Some`).
    pub has_max_characters: FfiBool,
    /// Whether `visible_lines` is engaged (`0 = None`, `1 = Some`).
    pub has_visible_lines: FfiBool,
    /// Whether `visible_width` is engaged (`0 = None`, `1 = Some`).
    pub has_visible_width: FfiBool,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 2],
}

impl_wire_abi_pod!(
    FfiEditableTextConfig,
    size = 36,
    align = 4,
    schema = [
        cursor_margin: [f32; 2] => "[f32;2]",
        cursor_width: f32 => "f32",
        cursor_blink_period_secs: f32 => "f32",
        visible_lines: f32 => "f32",
        visible_width: f32 => "f32",
        max_characters: u32 => "u32",
        interactive: FfiBool => "FfiBool",
        allow_newlines: FfiBool => "FfiBool",
        read_write_mode: u8 => "u8",
        has_max_characters: FfiBool => "FfiBool",
        has_visible_lines: FfiBool => "FfiBool",
        has_visible_width: FfiBool => "FfiBool",
        reserved0: [u8; 2] => "[u8;2]",
    ]
);

#[allow(missing_docs)]
impl FfiEditableTextConfig {
    pub const MODE_EDITABLE: u8 = 0;
    pub const MODE_READ_ONLY: u8 = 1;
    pub const MODE_STATIC: u8 = 2;
}

impl Default for FfiEditableTextConfig {
    #[inline]
    fn default() -> Self {
        Self {
            cursor_margin: [0.2, 0.2],
            cursor_width: 0.2,
            cursor_blink_period_secs: 1.0,
            visible_lines: 1.0,
            visible_width: 0.0,
            max_characters: 0,
            interactive: FfiBool::TRUE,
            allow_newlines: FfiBool::FALSE,
            read_write_mode: Self::MODE_EDITABLE,
            has_max_characters: FfiBool::FALSE,
            has_visible_lines: FfiBool::TRUE,
            has_visible_width: FfiBool::FALSE,
            reserved0: [0; 2],
        }
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s `InlineBox` (`"bamabi::ui::FfiInlineBox"`,
/// 12 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiInlineBox {
    /// Size of the inline box in logical pixels `[width, height]`.
    pub size: [f32; 2],
    /// Flow mode (`0 = OutOfFlow`, `1 = InFlow`), matching `InlineBoxKind`.
    pub kind: u8,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
}

impl_wire_abi_pod!(
    FfiInlineBox,
    size = 12,
    align = 4,
    schema = [
        size: [f32; 2] => "[f32;2]",
        kind: u8 => "u8",
        reserved0: [u8; 3] => "[u8;3]",
    ]
);

impl FfiInlineBox {
    /// `InlineBoxKind::OutOfFlow` (`0`).
    pub const KIND_OUT_OF_FLOW: u8 = 0;
    /// `InlineBoxKind::InFlow` (`1`).
    pub const KIND_IN_FLOW: u8 = 1;

    /// Constructs an in-flow [`FfiInlineBox`] with logical pixel dimensions `(width, height)`.
    #[inline]
    pub const fn in_flow(width: f32, height: f32) -> Self {
        Self {
            size: [width, height],
            kind: Self::KIND_IN_FLOW,
            reserved0: [0; 3],
        }
    }

    /// Constructs an out-of-flow [`FfiInlineBox`] with logical pixel dimensions `(width, height)`.
    #[inline]
    pub const fn out_of_flow(width: f32, height: f32) -> Self {
        Self {
            size: [width, height],
            kind: Self::KIND_OUT_OF_FLOW,
            reserved0: [0; 3],
        }
    }
}

impl Default for FfiInlineBox {
    #[inline]
    fn default() -> Self {
        Self::out_of_flow(0.0, 0.0)
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s `InlineImage`
/// (`"bamabi::ui::FfiInlineImage"`, 60 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiInlineImage {
    /// Image texture handle (`FfiAssetHandle::NONE` resolves to `TRANSPARENT_IMAGE_HANDLE`).
    pub image: FfiAssetHandle,
    /// Multiplicative tint color (`InlineImage::color`).
    pub color: FfiRgba,
    /// Fixed width in logical pixels when `has_width` is non-zero.
    pub width: f32,
    /// Fixed height in logical pixels when `has_height` is non-zero.
    pub height: f32,
    /// Whether `width` is engaged (`0 = None`, `1 = Some`).
    pub has_width: FfiBool,
    /// Whether `height` is engaged (`0 = None`, `1 = Some`).
    pub has_height: FfiBool,
    /// Flip horizontally (`InlineImage::flip_x`).
    pub flip_x: FfiBool,
    /// Flip vertically (`InlineImage::flip_y`).
    pub flip_y: FfiBool,
}

impl_wire_abi_pod!(
    FfiInlineImage,
    size = 60,
    align = 4,
    schema = [
        image: FfiAssetHandle => "FfiAssetHandle",
        color: FfiRgba => "FfiRgba",
        width: f32 => "f32",
        height: f32 => "f32",
        has_width: FfiBool => "FfiBool",
        has_height: FfiBool => "FfiBool",
        flip_x: FfiBool => "FfiBool",
        flip_y: FfiBool => "FfiBool",
    ]
);

impl Default for FfiInlineImage {
    #[inline]
    fn default() -> Self {
        Self {
            image: FfiAssetHandle::NONE,
            color: FfiRgba::WHITE,
            width: 0.0,
            height: 0.0,
            has_width: FfiBool::FALSE,
            has_height: FfiBool::FALSE,
            flip_x: FfiBool::FALSE,
            flip_y: FfiBool::FALSE,
        }
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s headless `TabList`
/// (`"bamabi::ui::FfiTabList"`, 4 bytes, align 1).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiTabList {
    /// Navigation orientation (`0 = Horizontal`, `1 = Vertical`).
    pub orientation: u8,
    /// Selection activation policy (`0 = Manual`, `1 = Automatic`).
    pub activation: u8,
    /// When non-zero, automatically applies emitted `ValueChange<Option<Entity>>` updates onto
    /// `SelectedTab` (mirroring `tablist_self_update`).
    pub self_update: FfiBool,
    /// When non-zero, attaches `InteractionDisabled` onto the tab list.
    pub disabled: FfiBool,
}

impl_wire_abi_pod!(
    FfiTabList,
    size = 4,
    align = 1,
    schema = [
        orientation: u8 => "u8",
        activation: u8 => "u8",
        self_update: FfiBool => "FfiBool",
        disabled: FfiBool => "FfiBool",
    ]
);

#[allow(missing_docs)]
impl FfiTabList {
    pub const ORIENTATION_HORIZONTAL: u8 = 0;
    pub const ORIENTATION_VERTICAL: u8 = 1;
    pub const ACTIVATION_MANUAL: u8 = 0;
    pub const ACTIVATION_AUTOMATIC: u8 = 1;
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s headless `Tab`
/// (`"bamabi::ui::FfiTab"`, 4 bytes, align 1).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiTab {
    /// When non-zero, attaches `InteractionDisabled` onto this tab.
    pub disabled: FfiBool,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
}

impl_wire_abi_pod!(
    FfiTab,
    size = 4,
    align = 1,
    schema = [
        disabled: FfiBool => "FfiBool",
        reserved0: [u8; 3] => "[u8;3]",
    ]
);

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s headless `SelectedTab`
/// (`"bamabi::ui::FfiSelectedTab"`, 8 bytes, align 8 on 64-bit / 4 on i686).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiSelectedTab {
    /// Currently selected tab entity (`FfiEntity::NULL` represents `None`).
    pub selected: FfiEntity,
}

impl_wire_abi_pod!(
    FfiSelectedTab,
    size = 8,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [selected: FfiEntity => "FfiEntity"]
);

impl FfiSelectedTab {
    /// Constructs a [`FfiSelectedTab`] from an optional [`FfiEntity`] (`None` -> `FfiEntity::NULL`).
    #[inline]
    pub const fn from_option(entity: Option<FfiEntity>) -> Self {
        Self {
            selected: match entity {
                Some(e) => e,
                None => FfiEntity::NULL,
            },
        }
    }

    /// Converts this descriptor into an `Option<FfiEntity>` (`FfiEntity::NULL` -> `None`).
    #[inline]
    pub const fn to_option(self) -> Option<FfiEntity> {
        self.selected_opt()
    }

    /// Returns `Some(entity)` when a tab is selected, or `None` when [`FfiEntity::NULL`].
    #[inline]
    pub const fn selected_opt(&self) -> Option<FfiEntity> {
        if self.selected.is_null() {
            None
        } else {
            Some(self.selected)
        }
    }
}

/// Zero-padding `#[repr(C)]` unified `EntityEvent` bridging `bevy_ui_widgets::ValueChange<T>`
/// (`"bamabi::ui::FfiValueChange"`, 56 bytes, align 8 on 64-bit / 4 on i686).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiValueChange {
    /// Source widget entity that emitted this `ValueChange` (`EntityEvent::event_target`).
    pub source: FfiEntity,
    /// Entity value for `KIND_ENTITY` and `KIND_OPTIONAL_ENTITY` (`FfiEntity::NULL` = `None`).
    pub entity_value: FfiEntity,
    /// Floating-point payload (`f64`, also populated for `KIND_F32`).
    pub f64_value: f64,
    /// Signed 64-bit integer payload (`i64`, also populated for `KIND_I32`).
    pub i64_value: i64,
    /// sRGBA color payload for `KIND_COLOR`.
    pub color_value: FfiRgba,
    /// 32-bit floating-point payload for `KIND_F32`.
    pub f32_value: f32,
    /// Discriminant tag identifying the active payload variant (`KIND_*`).
    pub kind: u8,
    /// Boolean payload for `KIND_BOOL`.
    pub bool_value: FfiBool,
    /// Mirrors `ValueChange::is_final`.
    pub is_final: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiValueChange,
    size = 56,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        source: FfiEntity => "FfiEntity",
        entity_value: FfiEntity => "FfiEntity",
        f64_value: f64 => "f64",
        i64_value: i64 => "i64",
        color_value: FfiRgba => "FfiRgba",
        f32_value: f32 => "f32",
        kind: u8 => "u8",
        bool_value: FfiBool => "FfiBool",
        is_final: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

#[allow(missing_docs)]
impl FfiValueChange {
    pub const KIND_ENTITY: u8 = 0;
    pub const KIND_OPTIONAL_ENTITY: u8 = 1;
    pub const KIND_F32: u8 = 2;
    pub const KIND_BOOL: u8 = 3;
    pub const KIND_F64: u8 = 4;
    pub const KIND_I32: u8 = 5;
    pub const KIND_I64: u8 = 6;
    pub const KIND_COLOR: u8 = 7;

    /// Constructs a [`FfiValueChange`] carrying an `Entity` payload (`KIND_ENTITY`).
    #[inline]
    pub const fn from_entity(source: FfiEntity, value: FfiEntity, is_final: bool) -> Self {
        Self {
            source,
            entity_value: value,
            f64_value: 0.0,
            i64_value: 0,
            color_value: FfiRgba::TRANSPARENT,
            f32_value: 0.0,
            kind: Self::KIND_ENTITY,
            bool_value: FfiBool::FALSE,
            is_final: FfiBool::from_bool(is_final),
            reserved0: 0,
        }
    }

    /// Constructs a [`FfiValueChange`] carrying an `Option<Entity>` payload (`KIND_OPTIONAL_ENTITY`).
    #[inline]
    pub const fn from_optional_entity(
        source: FfiEntity,
        value: Option<FfiEntity>,
        is_final: bool,
    ) -> Self {
        Self {
            source,
            entity_value: match value {
                Some(e) => e,
                None => FfiEntity::NULL,
            },
            f64_value: 0.0,
            i64_value: 0,
            color_value: FfiRgba::TRANSPARENT,
            f32_value: 0.0,
            kind: Self::KIND_OPTIONAL_ENTITY,
            bool_value: FfiBool::FALSE,
            is_final: FfiBool::from_bool(is_final),
            reserved0: 0,
        }
    }

    /// Constructs a [`FfiValueChange`] carrying an `f32` payload (`KIND_F32`).
    #[inline]
    pub const fn from_f32(source: FfiEntity, value: f32, is_final: bool) -> Self {
        Self {
            source,
            entity_value: FfiEntity::NULL,
            f64_value: value as f64,
            i64_value: 0,
            color_value: FfiRgba::TRANSPARENT,
            f32_value: value,
            kind: Self::KIND_F32,
            bool_value: FfiBool::FALSE,
            is_final: FfiBool::from_bool(is_final),
            reserved0: 0,
        }
    }

    /// Constructs a [`FfiValueChange`] carrying a `bool` payload (`KIND_BOOL`).
    #[inline]
    pub const fn from_bool(source: FfiEntity, value: bool, is_final: bool) -> Self {
        Self {
            source,
            entity_value: FfiEntity::NULL,
            f64_value: 0.0,
            i64_value: 0,
            color_value: FfiRgba::TRANSPARENT,
            f32_value: 0.0,
            kind: Self::KIND_BOOL,
            bool_value: FfiBool::from_bool(value),
            is_final: FfiBool::from_bool(is_final),
            reserved0: 0,
        }
    }

    /// Constructs a [`FfiValueChange`] carrying an `f64` payload (`KIND_F64`).
    #[inline]
    pub const fn from_f64(source: FfiEntity, value: f64, is_final: bool) -> Self {
        Self {
            source,
            entity_value: FfiEntity::NULL,
            f64_value: value,
            i64_value: 0,
            color_value: FfiRgba::TRANSPARENT,
            f32_value: value as f32,
            kind: Self::KIND_F64,
            bool_value: FfiBool::FALSE,
            is_final: FfiBool::from_bool(is_final),
            reserved0: 0,
        }
    }

    /// Constructs a [`FfiValueChange`] carrying an `i32` payload (`KIND_I32`).
    #[inline]
    pub const fn from_i32(source: FfiEntity, value: i32, is_final: bool) -> Self {
        Self {
            source,
            entity_value: FfiEntity::NULL,
            f64_value: value as f64,
            i64_value: value as i64,
            color_value: FfiRgba::TRANSPARENT,
            f32_value: value as f32,
            kind: Self::KIND_I32,
            bool_value: FfiBool::FALSE,
            is_final: FfiBool::from_bool(is_final),
            reserved0: 0,
        }
    }

    /// Constructs a [`FfiValueChange`] carrying an `i64` payload (`KIND_I64`).
    #[inline]
    pub const fn from_i64(source: FfiEntity, value: i64, is_final: bool) -> Self {
        Self {
            source,
            entity_value: FfiEntity::NULL,
            f64_value: value as f64,
            i64_value: value,
            color_value: FfiRgba::TRANSPARENT,
            f32_value: value as f32,
            kind: Self::KIND_I64,
            bool_value: FfiBool::FALSE,
            is_final: FfiBool::from_bool(is_final),
            reserved0: 0,
        }
    }

    /// Constructs a [`FfiValueChange`] carrying an sRGBA [`FfiRgba`] color payload (`KIND_COLOR`).
    #[inline]
    pub const fn from_color(source: FfiEntity, color: FfiRgba, is_final: bool) -> Self {
        Self {
            source,
            entity_value: FfiEntity::NULL,
            f64_value: 0.0,
            i64_value: 0,
            color_value: color,
            f32_value: 0.0,
            kind: Self::KIND_COLOR,
            bool_value: FfiBool::FALSE,
            is_final: FfiBool::from_bool(is_final),
            reserved0: 0,
        }
    }
}

impl Default for FfiValueChange {
    #[inline]
    fn default() -> Self {
        Self {
            source: FfiEntity::NULL,
            entity_value: FfiEntity::NULL,
            f64_value: 0.0,
            i64_value: 0,
            color_value: FfiRgba::TRANSPARENT,
            f32_value: 0.0,
            kind: Self::KIND_ENTITY,
            bool_value: FfiBool::FALSE,
            is_final: FfiBool::TRUE,
            reserved0: 0,
        }
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20`'s `bevy_feathers::theme::ThemeContext`
/// (`"bamabi::ui::FfiThemeContext"`, 4 bytes, align 1).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiThemeContext {
    /// Surface elevation discriminant (`0 = Base`, `1 = Higher`, `2 = Highest`, `3 = Floating`).
    pub surface_level: u8,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
}

impl_wire_abi_pod!(
    FfiThemeContext,
    size = 4,
    align = 1,
    schema = [
        surface_level: u8 => "u8",
        reserved0: [u8; 3] => "[u8;3]",
    ]
);

impl FfiThemeContext {
    /// Base surface elevation (`0`).
    pub const BASE: Self = Self {
        surface_level: 0,
        reserved0: [0; 3],
    };
    /// Higher surface elevation (`1`).
    pub const HIGHER: Self = Self {
        surface_level: 1,
        reserved0: [0; 3],
    };
    /// Highest surface elevation (`2`).
    pub const HIGHEST: Self = Self {
        surface_level: 2,
        reserved0: [0; 3],
    };
    /// Floating surface elevation (`3`).
    pub const FLOATING: Self = Self {
        surface_level: 3,
        reserved0: [0; 3],
    };
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20` `bevy_feathers` `ColorInput`
/// (`"bamabi::ui::FfiFeathersColorInput"`, 24 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiFeathersColorInput {
    /// Current sRGBA color synchronized with `ColorInputValue`.
    pub color: FfiRgba,
    /// Fraction/percentage of the swatch showing the opaque color preview.
    pub opaque_color_percentage: f32,
    /// Color picker editing mode (`0 = Wheel`, `1 = RGPlane`, `2 = HSPlane`).
    pub mode: u8,
    /// When non-zero, also applies `mode` onto the global `ColorInputSettings` resource.
    pub apply_mode_to_settings: FfiBool,
    /// When non-zero, attaches `InteractionDisabled`.
    pub disabled: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiFeathersColorInput,
    size = 24,
    align = 4,
    schema = [
        color: FfiRgba => "FfiRgba",
        opaque_color_percentage: f32 => "f32",
        mode: u8 => "u8",
        apply_mode_to_settings: FfiBool => "FfiBool",
        disabled: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

#[allow(missing_docs)]
impl FfiFeathersColorInput {
    pub const MODE_WHEEL: u8 = 0;
    pub const MODE_RG_PLANE: u8 = 1;
    pub const MODE_HS_PLANE: u8 = 2;
}

impl Default for FfiFeathersColorInput {
    #[inline]
    fn default() -> Self {
        Self {
            color: FfiRgba::WHITE,
            opaque_color_percentage: 0.0,
            mode: Self::MODE_WHEEL,
            apply_mode_to_settings: FfiBool::FALSE,
            disabled: FfiBool::FALSE,
            reserved0: 0,
        }
    }
}

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20` `bevy_feathers` `Dropdown` / `Select`
/// (`"bamabi::ui::FfiFeathersDropdown"`, 24 bytes, align 8 on 64-bit / 4 on i686).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FfiFeathersDropdown {
    /// Selected option row entity (`FfiEntity::NULL` = none).
    pub selected_row: FfiEntity,
    /// Selected option index (synchronized with `OptionIndex` when `has_selected_index` is set).
    pub selected_index: u32,
    /// Maximum visible rows before scrolling (`FeathersSelectProps::max_visible`, default `8`).
    pub max_visible: u32,
    /// Whether `selected_index` is engaged (`0 = None`, `1 = Some`).
    pub has_selected_index: FfiBool,
    /// Rounded-corner preset matching `RoundedCorners` (`0 = None`, `1 = All`, ..., `9 = Left`).
    pub rounded_corners: u8,
    /// When non-zero, attaches `InteractionDisabled`.
    pub disabled: FfiBool,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 5],
}

impl_wire_abi_pod!(
    FfiFeathersDropdown,
    size = 24,
    align = core::mem::align_of::<FfiEntity>(),
    schema = [
        selected_row: FfiEntity => "FfiEntity",
        selected_index: u32 => "u32",
        max_visible: u32 => "u32",
        has_selected_index: FfiBool => "FfiBool",
        rounded_corners: u8 => "u8",
        disabled: FfiBool => "FfiBool",
        reserved0: [u8; 5] => "[u8;5]",
    ]
);

#[allow(missing_docs)]
impl FfiFeathersDropdown {
    pub const CORNERS_NONE: u8 = 0;
    pub const CORNERS_ALL: u8 = 1;
    pub const CORNERS_TOP_LEFT: u8 = 2;
    pub const CORNERS_TOP_RIGHT: u8 = 3;
    pub const CORNERS_BOTTOM_RIGHT: u8 = 4;
    pub const CORNERS_BOTTOM_LEFT: u8 = 5;
    pub const CORNERS_TOP: u8 = 6;
    pub const CORNERS_RIGHT: u8 = 7;
    pub const CORNERS_BOTTOM: u8 = 8;
    pub const CORNERS_LEFT: u8 = 9;
}

impl Default for FfiFeathersDropdown {
    #[inline]
    fn default() -> Self {
        Self {
            selected_row: FfiEntity::NULL,
            selected_index: 0,
            max_visible: 8,
            has_selected_index: FfiBool::FALSE,
            rounded_corners: Self::CORNERS_ALL,
            disabled: FfiBool::FALSE,
            reserved0: [0; 5],
        }
    }
}

/// Alias mirroring `bamabi_bridges::ui::FfiFeathersSelect`.
pub type FfiFeathersSelect = FfiFeathersDropdown;

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20` `bevy_feathers` `ListView` / scrollable list
/// (`"bamabi::ui::FfiFeathersScrollableList"`, 12 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiFeathersScrollableList {
    /// Scrollbar gutter inset (`ScrollbarGutter`, default `Val::Px(14.0)`).
    pub scrollbar_gutter: FfiVal,
    /// Scroll orientation (`0 = Horizontal`, `1 = Vertical`, default `Vertical`).
    pub orientation: u8,
    /// When non-zero, attaches headless `ListBox` selection management.
    pub selectable: FfiBool,
    /// When non-zero, attaches headless `ScrollArea` scrolling behavior.
    pub scrollable: FfiBool,
    /// When non-zero, attaches `InteractionDisabled`.
    pub disabled: FfiBool,
}

impl_wire_abi_pod!(
    FfiFeathersScrollableList,
    size = 12,
    align = 4,
    schema = [
        scrollbar_gutter: FfiVal => "FfiVal",
        orientation: u8 => "u8",
        selectable: FfiBool => "FfiBool",
        scrollable: FfiBool => "FfiBool",
        disabled: FfiBool => "FfiBool",
    ]
);

#[allow(missing_docs)]
impl FfiFeathersScrollableList {
    pub const ORIENTATION_HORIZONTAL: u8 = 0;
    pub const ORIENTATION_VERTICAL: u8 = 1;
}

impl Default for FfiFeathersScrollableList {
    #[inline]
    fn default() -> Self {
        Self {
            scrollbar_gutter: FfiVal::px(14.0),
            orientation: Self::ORIENTATION_VERTICAL,
            selectable: FfiBool::TRUE,
            scrollable: FfiBool::TRUE,
            disabled: FfiBool::FALSE,
        }
    }
}

/// Alias mirroring `bamabi_bridges::ui::FfiFeathersListView`.
pub type FfiFeathersListView = FfiFeathersScrollableList;

/// Zero-padding `#[repr(C)]` projection of a Feathers list row (`"bamabi::ui::FfiFeathersListRow"`,
/// 8 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiFeathersListRow {
    /// Option index synchronized with `OptionIndex` when `has_option_index` is non-zero.
    pub option_index: u32,
    /// Whether `option_index` should be attached as `OptionIndex`.
    pub has_option_index: FfiBool,
    /// When non-zero, attaches `Selected`; when zero, removes `Selected`.
    pub selected: FfiBool,
    /// When non-zero, attaches `InteractionDisabled`.
    pub disabled: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiFeathersListRow,
    size = 8,
    align = 4,
    schema = [
        option_index: u32 => "u32",
        has_option_index: FfiBool => "FfiBool",
        selected: FfiBool => "FfiBool",
        disabled: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

/// Zero-padding `#[repr(C)]` projection of Bevy `0.20` `bevy_feathers` `NumberInput`
/// (`"bamabi::ui::FfiFeathersNumberInput"`, 72 bytes, align 8 on 64-bit / 4 on i686).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiFeathersNumberInput {
    /// Current floating-point value for `FORMAT_F32` / `FORMAT_F64`.
    pub value_f64: f64,
    /// Current integer value for `FORMAT_I32` / `FORMAT_I64`.
    pub value_i64: i64,
    /// Soft scrub-drag lower bound (`SoftLimit`).
    pub soft_min: f64,
    /// Soft scrub-drag upper bound (`SoftLimit`).
    pub soft_max: f64,
    /// Hard clamp/wrap lower bound (`HardLimit`).
    pub hard_min: f64,
    /// Hard clamp/wrap upper bound (`HardLimit`).
    pub hard_max: f64,
    /// Increment/decrement and scrub heuristic step size (`NumberInputStep`).
    pub step: f64,
    /// Decimal precision during scrubbing (`NumberInputPrecision`, default `2`).
    pub precision: i32,
    /// Numeric format (`0 = F32`, `1 = F64`, `2 = I32`, `3 = I64`).
    pub format: u8,
    /// When non-zero, attaches `SoftLimit` defining the scrub-drag range.
    pub has_soft_limit: FfiBool,
    /// When non-zero, attaches `HardLimit` defining the hard clamp/wrap range.
    pub has_hard_limit: FfiBool,
    /// When non-zero, sets `NumberInputWrap::Wrap`; otherwise `NumberInputWrap::NoWrap`.
    pub wrap: FfiBool,
    /// When non-zero, attaches `InteractionDisabled`.
    pub disabled: FfiBool,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 7],
}

impl_wire_abi_pod!(
    FfiFeathersNumberInput,
    size = 72,
    align = core::mem::align_of::<f64>(),
    schema = [
        value_f64: f64 => "f64",
        value_i64: i64 => "i64",
        soft_min: f64 => "f64",
        soft_max: f64 => "f64",
        hard_min: f64 => "f64",
        hard_max: f64 => "f64",
        step: f64 => "f64",
        precision: i32 => "i32",
        format: u8 => "u8",
        has_soft_limit: FfiBool => "FfiBool",
        has_hard_limit: FfiBool => "FfiBool",
        wrap: FfiBool => "FfiBool",
        disabled: FfiBool => "FfiBool",
        reserved0: [u8; 7] => "[u8;7]",
    ]
);

#[allow(missing_docs)]
impl FfiFeathersNumberInput {
    pub const FORMAT_F32: u8 = 0;
    pub const FORMAT_F64: u8 = 1;
    pub const FORMAT_I32: u8 = 2;
    pub const FORMAT_I64: u8 = 3;

    /// Constructs an `f32` scrubbable number-input descriptor matching `bamabi_bridges::ui`.
    #[inline]
    pub fn f32(value: f32) -> Self {
        Self {
            value_f64: f64::from(value),
            format: Self::FORMAT_F32,
            ..Self::default()
        }
    }

    /// Constructs an `f64` scrubbable number-input descriptor matching `bamabi_bridges::ui`.
    #[inline]
    pub fn f64(value: f64) -> Self {
        Self {
            value_f64: value,
            format: Self::FORMAT_F64,
            ..Self::default()
        }
    }

    /// Constructs an `i32` scrubbable number-input descriptor (`precision = 0`).
    #[inline]
    pub fn i32(value: i32) -> Self {
        Self {
            value_i64: i64::from(value),
            format: Self::FORMAT_I32,
            precision: 0,
            ..Self::default()
        }
    }

    /// Constructs an `i64` scrubbable number-input descriptor (`precision = 0`).
    #[inline]
    pub fn i64(value: i64) -> Self {
        Self {
            value_i64: value,
            format: Self::FORMAT_I64,
            precision: 0,
            ..Self::default()
        }
    }

    /// Configures a scrub-drag `SoftLimit` range `(min, max)`.
    #[inline]
    #[must_use]
    pub const fn with_soft_limit(mut self, min: f64, max: f64) -> Self {
        self.soft_min = min;
        self.soft_max = max;
        self.has_soft_limit = FfiBool::TRUE;
        self
    }

    /// Configures a hard clamp/wrap `HardLimit` range `(min, max)`.
    #[inline]
    #[must_use]
    pub const fn with_hard_limit(mut self, min: f64, max: f64) -> Self {
        self.hard_min = min;
        self.hard_max = max;
        self.has_hard_limit = FfiBool::TRUE;
        self
    }
}

impl Default for FfiFeathersNumberInput {
    #[inline]
    fn default() -> Self {
        Self {
            value_f64: 0.0,
            value_i64: 0,
            soft_min: 0.0,
            soft_max: 0.0,
            hard_min: 0.0,
            hard_max: 0.0,
            step: 1.0,
            precision: 2,
            format: Self::FORMAT_F32,
            has_soft_limit: FfiBool::FALSE,
            has_hard_limit: FfiBool::FALSE,
            wrap: FfiBool::FALSE,
            disabled: FfiBool::FALSE,
            reserved0: [0; 7],
        }
    }
}
