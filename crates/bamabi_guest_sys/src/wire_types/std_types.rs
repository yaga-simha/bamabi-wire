//! Core Bevy `0.20` wire projection types (`Transform`, `GlobalTransform`, `Visibility`,
//! `InheritedVisibility`, `ViewVisibility`, `Color`, `Time<*>`, `ButtonInput<*>`, mouse motion,
//! mouse wheel, and `Gamepad`).

use bamabi_sys::FfiBool;
use core::time::Duration;

// --- Internal deterministic vector / quaternion math helpers ---

#[inline]
fn vec3_add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[inline]
fn vec3_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn vec3_neg(v: [f32; 3]) -> [f32; 3] {
    [-v[0], -v[1], -v[2]]
}

#[inline]
fn vec3_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[inline]
fn vec3_normalize_or(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let len_sq = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if len_sq > 1e-12 && len_sq.is_finite() {
        let inv = len_sq.sqrt().recip();
        [v[0] * inv, v[1] * inv, v[2] * inv]
    } else {
        fallback
    }
}

#[inline]
fn quat_normalize(q: [f32; 4]) -> [f32; 4] {
    let len_sq = q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3];
    if len_sq > 1e-12 && len_sq.is_finite() {
        let inv = len_sq.sqrt().recip();
        [q[0] * inv, q[1] * inv, q[2] * inv, q[3] * inv]
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

#[inline]
fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

#[inline]
fn quat_rotate_vec3(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let qv = [q[0], q[1], q[2]];
    let c = vec3_cross(qv, v);
    let t = [2.0 * c[0], 2.0 * c[1], 2.0 * c[2]];
    let u = vec3_cross(qv, t);
    [
        v[0] + q[3] * t[0] + u[0],
        v[1] + q[3] * t[1] + u[1],
        v[2] + q[3] * t[2] + u[2],
    ]
}

#[inline]
fn quat_from_axis_angle(axis: [f32; 3], angle: f32) -> [f32; 4] {
    let norm = vec3_normalize_or(axis, [0.0, 1.0, 0.0]);
    let (s, c) = (angle * 0.5).sin_cos();
    [norm[0] * s, norm[1] * s, norm[2] * s, c]
}

#[inline]
fn quat_from_orthonormal_cols(x: [f32; 3], y: [f32; 3], z: [f32; 3]) -> [f32; 4] {
    // Stable branch-selected matrix-to-quaternion extraction on column-major 3x3 M = [x, y, z].
    let m00 = x[0];
    let m10 = x[1];
    let m20 = x[2];
    let m01 = y[0];
    let m11 = y[1];
    let m21 = y[2];
    let m02 = z[0];
    let m12 = z[1];
    let m22 = z[2];
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        let inv = s.recip();
        [
            (m21 - m12) * inv,
            (m02 - m20) * inv,
            (m10 - m01) * inv,
            0.25 * s,
        ]
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        let inv = s.recip();
        [
            0.25 * s,
            (m01 + m10) * inv,
            (m02 + m20) * inv,
            (m21 - m12) * inv,
        ]
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        let inv = s.recip();
        [
            (m01 + m10) * inv,
            0.25 * s,
            (m12 + m21) * inv,
            (m02 - m20) * inv,
        ]
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        let inv = s.recip();
        [
            (m02 + m20) * inv,
            (m12 + m21) * inv,
            0.25 * s,
            (m10 - m01) * inv,
        ]
    };
    quat_normalize(q)
}

#[inline]
fn srgb_channel_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[inline]
fn linear_channel_to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Fixed-layout `#[repr(C)]` wire projection of Bevy `Transform`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiTransform {
    /// Local translation `[x, y, z]`.
    pub translation: [f32; 3],
    /// Local rotation quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    /// Local non-uniform scale `[x, y, z]`.
    pub scale: [f32; 3],
}

impl_wire_abi_pod!(
    FfiTransform,
    size = 40,
    align = 4,
    fields = [[f32; 3], [f32; 4], [f32; 3]]
);

impl FfiTransform {
    /// Identity transform (`translation = [0; 3]`, `rotation = [0, 0, 0, 1]`, `scale = [1; 3]`).
    pub const IDENTITY: Self = Self {
        translation: [0.0, 0.0, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    };

    /// Constructs a transform at `(x, y, z)` with identity rotation and unit scale.
    #[inline]
    pub const fn from_xyz(x: f32, y: f32, z: f32) -> Self {
        Self {
            translation: [x, y, z],
            ..Self::IDENTITY
        }
    }

    /// Constructs a transform from a `[x, y, z]` translation vector.
    #[inline]
    pub const fn from_translation(translation: [f32; 3]) -> Self {
        Self {
            translation,
            ..Self::IDENTITY
        }
    }

    /// Constructs a transform from an `[x, y, z, w]` quaternion.
    #[inline]
    pub const fn from_rotation(rotation: [f32; 4]) -> Self {
        Self {
            rotation,
            ..Self::IDENTITY
        }
    }

    /// Constructs a transform rotated around the X axis by `angle` radians.
    #[inline]
    pub fn from_rotation_x(angle: f32) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self::from_rotation([s, 0.0, 0.0, c])
    }

    /// Constructs a transform rotated around the Y axis by `angle` radians.
    #[inline]
    pub fn from_rotation_y(angle: f32) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self::from_rotation([0.0, s, 0.0, c])
    }

    /// Constructs a transform rotated around the Z axis by `angle` radians.
    #[inline]
    pub fn from_rotation_z(angle: f32) -> Self {
        let (s, c) = (angle * 0.5).sin_cos();
        Self::from_rotation([0.0, 0.0, s, c])
    }

    /// Constructs a transform from an `[x, y, z]` scale vector.
    #[inline]
    pub const fn from_scale(scale: [f32; 3]) -> Self {
        Self {
            scale,
            ..Self::IDENTITY
        }
    }

    /// Constructs a transform with uniform scale `[s, s, s]`.
    #[inline]
    pub const fn from_scale_splat(s: f32) -> Self {
        Self::from_scale([s, s, s])
    }

    /// Returns a copy of this transform with `translation` replaced.
    #[inline]
    #[must_use]
    pub const fn with_translation(mut self, translation: [f32; 3]) -> Self {
        self.translation = translation;
        self
    }

    /// Returns a copy of this transform with `rotation` replaced.
    #[inline]
    #[must_use]
    pub const fn with_rotation(mut self, rotation: [f32; 4]) -> Self {
        self.rotation = rotation;
        self
    }

    /// Returns a copy of this transform with `scale` replaced.
    #[inline]
    #[must_use]
    pub const fn with_scale(mut self, scale: [f32; 3]) -> Self {
        self.scale = scale;
        self
    }

    /// Returns the unit local `+X` axis after applying `self.rotation`.
    #[inline]
    pub fn local_x(&self) -> [f32; 3] {
        quat_rotate_vec3(self.rotation, [1.0, 0.0, 0.0])
    }

    /// Returns the unit local `+Y` axis after applying `self.rotation`.
    #[inline]
    pub fn local_y(&self) -> [f32; 3] {
        quat_rotate_vec3(self.rotation, [0.0, 1.0, 0.0])
    }

    /// Returns the unit local `+Z` axis after applying `self.rotation`.
    #[inline]
    pub fn local_z(&self) -> [f32; 3] {
        quat_rotate_vec3(self.rotation, [0.0, 0.0, 1.0])
    }

    /// Returns the unit right direction (`+X` in Bevy right-handed coordinates).
    #[inline]
    pub fn right(&self) -> [f32; 3] {
        self.local_x()
    }

    /// Returns the unit left direction (`-X`).
    #[inline]
    pub fn left(&self) -> [f32; 3] {
        vec3_neg(self.local_x())
    }

    /// Returns the unit up direction (`+Y`).
    #[inline]
    pub fn up(&self) -> [f32; 3] {
        self.local_y()
    }

    /// Returns the unit down direction (`-Y`).
    #[inline]
    pub fn down(&self) -> [f32; 3] {
        vec3_neg(self.local_y())
    }

    /// Returns the unit back direction (`+Z` in Bevy right-handed coordinates).
    #[inline]
    pub fn back(&self) -> [f32; 3] {
        self.local_z()
    }

    /// Returns the unit forward direction (`-Z` in Bevy right-handed coordinates).
    #[inline]
    pub fn forward(&self) -> [f32; 3] {
        vec3_neg(self.local_z())
    }

    /// Rotates this transform by quaternion `rotation` in parent/world space (`rotation * self.rotation`).
    #[inline]
    pub fn rotate(&mut self, rotation: [f32; 4]) {
        self.rotation = quat_normalize(quat_mul(rotation, self.rotation));
    }

    /// Rotates this transform around parent/world `+X` by `angle` radians.
    #[inline]
    pub fn rotate_x(&mut self, angle: f32) {
        let (s, c) = (angle * 0.5).sin_cos();
        self.rotate([s, 0.0, 0.0, c]);
    }

    /// Rotates this transform around parent/world `+Y` by `angle` radians.
    #[inline]
    pub fn rotate_y(&mut self, angle: f32) {
        let (s, c) = (angle * 0.5).sin_cos();
        self.rotate([0.0, s, 0.0, c]);
    }

    /// Rotates this transform around parent/world `+Z` by `angle` radians.
    #[inline]
    pub fn rotate_z(&mut self, angle: f32) {
        let (s, c) = (angle * 0.5).sin_cos();
        self.rotate([0.0, 0.0, s, c]);
    }

    /// Rotates this transform around `axis` by `angle` radians in parent/world space.
    #[inline]
    pub fn rotate_axis(&mut self, axis: [f32; 3], angle: f32) {
        self.rotate(quat_from_axis_angle(axis, angle));
    }

    /// Rotates this transform by quaternion `rotation` in local space (`self.rotation * rotation`).
    #[inline]
    pub fn rotate_local(&mut self, rotation: [f32; 4]) {
        self.rotation = quat_normalize(quat_mul(self.rotation, rotation));
    }

    /// Rotates this transform around its local `+X` axis by `angle` radians.
    #[inline]
    pub fn rotate_local_x(&mut self, angle: f32) {
        let (s, c) = (angle * 0.5).sin_cos();
        self.rotate_local([s, 0.0, 0.0, c]);
    }

    /// Rotates this transform around its local `+Y` axis by `angle` radians.
    #[inline]
    pub fn rotate_local_y(&mut self, angle: f32) {
        let (s, c) = (angle * 0.5).sin_cos();
        self.rotate_local([0.0, s, 0.0, c]);
    }

    /// Rotates this transform around its local `+Z` axis by `angle` radians.
    #[inline]
    pub fn rotate_local_z(&mut self, angle: f32) {
        let (s, c) = (angle * 0.5).sin_cos();
        self.rotate_local([0.0, 0.0, s, c]);
    }

    /// Translates `self.translation` around `point` by quaternion `rotation` without changing orientation.
    #[inline]
    pub fn translate_around(&mut self, point: [f32; 3], rotation: [f32; 4]) {
        self.translation = vec3_add(
            point,
            quat_rotate_vec3(rotation, vec3_sub(self.translation, point)),
        );
    }

    /// Rotates this transform so its local `-Z` (`forward`) points along `direction` with `up`.
    #[inline]
    #[must_use]
    pub fn looking_to(mut self, direction: [f32; 3], up: [f32; 3]) -> Self {
        let back = vec3_normalize_or(vec3_neg(direction), [0.0, 0.0, 1.0]);
        let up_norm = vec3_normalize_or(up, [0.0, 1.0, 0.0]);
        let right_candidate = vec3_cross(up_norm, back);
        let right = if right_candidate[0] * right_candidate[0]
            + right_candidate[1] * right_candidate[1]
            + right_candidate[2] * right_candidate[2]
            > 1e-12
        {
            vec3_normalize_or(right_candidate, [1.0, 0.0, 0.0])
        } else {
            let alt = if back[1].abs() < 0.9 {
                [0.0, 1.0, 0.0]
            } else {
                [1.0, 0.0, 0.0]
            };
            vec3_normalize_or(vec3_cross(alt, back), [1.0, 0.0, 0.0])
        };
        let ortho_up = vec3_cross(back, right);
        self.rotation = quat_from_orthonormal_cols(right, ortho_up, back);
        self
    }

    /// Rotates this transform so its local `-Z` (`forward`) faces `target` with `up`.
    #[inline]
    #[must_use]
    pub fn looking_at(self, target: [f32; 3], up: [f32; 3]) -> Self {
        let dir = vec3_sub(target, self.translation);
        self.looking_to(dir, up)
    }

    /// Transforms a 3D point from local space into parent space (`T * R * S * point`).
    #[inline]
    pub fn transform_point(&self, point: [f32; 3]) -> [f32; 3] {
        let scaled = [
            point[0] * self.scale[0],
            point[1] * self.scale[1],
            point[2] * self.scale[2],
        ];
        vec3_add(self.translation, quat_rotate_vec3(self.rotation, scaled))
    }

    /// Composes `self` followed by child transform `rhs` (`self * rhs`).
    #[inline]
    #[must_use]
    pub fn mul_transform(&self, rhs: Self) -> Self {
        Self {
            translation: self.transform_point(rhs.translation),
            rotation: quat_normalize(quat_mul(self.rotation, rhs.rotation)),
            scale: [
                self.scale[0] * rhs.scale[0],
                self.scale[1] * rhs.scale[1],
                self.scale[2] * rhs.scale[2],
            ],
        }
    }
}

impl Default for FfiTransform {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Fixed-layout `#[repr(C)]` wire projection of Bevy `GlobalTransform` (`Affine3A` 3x4 columns).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiGlobalTransform {
    /// Column-major 3x4 affine basis (`x_axis`, `y_axis`, `z_axis`, `translation`).
    pub matrix_cols: [[f32; 3]; 4],
}

impl_wire_abi_pod!(
    FfiGlobalTransform,
    size = 48,
    align = 4,
    fields = [[[f32; 3]; 4]]
);

impl FfiGlobalTransform {
    /// Identity affine transform.
    pub const IDENTITY: Self = Self {
        matrix_cols: [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
        ],
    };

    /// Constructs a pure-translation [`FfiGlobalTransform`].
    #[inline]
    pub const fn from_translation(translation: [f32; 3]) -> Self {
        Self {
            matrix_cols: [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                translation,
            ],
        }
    }

    /// Returns the world-space translation column `[x, y, z]`.
    #[inline]
    pub const fn translation(&self) -> [f32; 3] {
        self.matrix_cols[3]
    }

    /// Returns the raw world-space X basis column (including scale).
    #[inline]
    pub const fn x_axis(&self) -> [f32; 3] {
        self.matrix_cols[0]
    }

    /// Returns the raw world-space Y basis column (including scale).
    #[inline]
    pub const fn y_axis(&self) -> [f32; 3] {
        self.matrix_cols[1]
    }

    /// Returns the raw world-space Z basis column (including scale).
    #[inline]
    pub const fn z_axis(&self) -> [f32; 3] {
        self.matrix_cols[2]
    }

    /// Returns the normalized world-space right direction (`+X`).
    #[inline]
    pub fn right(&self) -> [f32; 3] {
        vec3_normalize_or(self.matrix_cols[0], [1.0, 0.0, 0.0])
    }

    /// Returns the normalized world-space left direction (`-X`).
    #[inline]
    pub fn left(&self) -> [f32; 3] {
        vec3_neg(self.right())
    }

    /// Returns the normalized world-space up direction (`+Y`).
    #[inline]
    pub fn up(&self) -> [f32; 3] {
        vec3_normalize_or(self.matrix_cols[1], [0.0, 1.0, 0.0])
    }

    /// Returns the normalized world-space down direction (`-Y`).
    #[inline]
    pub fn down(&self) -> [f32; 3] {
        vec3_neg(self.up())
    }

    /// Returns the normalized world-space back direction (`+Z`).
    #[inline]
    pub fn back(&self) -> [f32; 3] {
        vec3_normalize_or(self.matrix_cols[2], [0.0, 0.0, 1.0])
    }

    /// Returns the normalized world-space forward direction (`-Z`).
    #[inline]
    pub fn forward(&self) -> [f32; 3] {
        vec3_neg(self.back())
    }

    /// Transforms a local-space 3D point into world space using this 3x4 affine matrix.
    #[inline]
    pub fn transform_point(&self, point: [f32; 3]) -> [f32; 3] {
        let c0 = self.matrix_cols[0];
        let c1 = self.matrix_cols[1];
        let c2 = self.matrix_cols[2];
        let c3 = self.matrix_cols[3];
        [
            c0[0] * point[0] + c1[0] * point[1] + c2[0] * point[2] + c3[0],
            c0[1] * point[0] + c1[1] * point[1] + c2[1] * point[2] + c3[1],
            c0[2] * point[0] + c1[2] * point[1] + c2[2] * point[2] + c3[2],
        ]
    }

    /// Expands this 3x4 affine matrix to a column-major 4x4 matrix.
    #[inline]
    pub const fn to_mat4(&self) -> [[f32; 4]; 4] {
        let c0 = self.matrix_cols[0];
        let c1 = self.matrix_cols[1];
        let c2 = self.matrix_cols[2];
        let c3 = self.matrix_cols[3];
        [
            [c0[0], c0[1], c0[2], 0.0],
            [c1[0], c1[1], c1[2], 0.0],
            [c2[0], c2[1], c2[2], 0.0],
            [c3[0], c3[1], c3[2], 1.0],
        ]
    }
}

impl Default for FfiGlobalTransform {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// Fixed-layout `#[repr(transparent)]` wire projection of Bevy `Visibility`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiVisibility(pub u8);

impl_wire_abi_pod!(FfiVisibility, size = 1, align = 1, fields = [u8]);

impl FfiVisibility {
    /// Inherit visibility from parent hierarchy (`0`).
    pub const INHERITED: Self = Self(0);
    /// Unconditionally hidden (`1`).
    pub const HIDDEN: Self = Self(1);
    /// Unconditionally visible (`2`).
    pub const VISIBLE: Self = Self(2);

    /// Returns `true` if `self == Self::INHERITED`.
    #[inline]
    pub const fn is_inherited(self) -> bool {
        self.0 == Self::INHERITED.0
    }

    /// Returns `true` if `self == Self::HIDDEN`.
    #[inline]
    pub const fn is_hidden(self) -> bool {
        self.0 == Self::HIDDEN.0
    }

    /// Returns `true` if `self == Self::VISIBLE`.
    #[inline]
    pub const fn is_visible(self) -> bool {
        self.0 == Self::VISIBLE.0
    }
}

/// Fixed-layout `#[repr(transparent)]` wire projection of Bevy `InheritedVisibility`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiInheritedVisibility(pub FfiBool);

impl_wire_abi_pod!(
    FfiInheritedVisibility,
    size = 1,
    align = 1,
    fields = [FfiBool]
);

impl FfiInheritedVisibility {
    /// Hidden inherited visibility (`false`).
    pub const HIDDEN: Self = Self(FfiBool::FALSE);
    /// Visible inherited visibility (`true`).
    pub const VISIBLE: Self = Self(FfiBool::TRUE);

    /// Returns `true` if the entity's inherited visibility is visible.
    #[inline]
    pub const fn get(self) -> bool {
        self.0.get()
    }
}

/// Fixed-layout `#[repr(transparent)]` wire projection of Bevy `ViewVisibility`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiViewVisibility(pub FfiBool);

impl_wire_abi_pod!(FfiViewVisibility, size = 1, align = 1, fields = [FfiBool]);

impl FfiViewVisibility {
    /// Entity is not visible in any active view (`false`).
    pub const HIDDEN: Self = Self(FfiBool::FALSE);
    /// Entity is visible in at least one active view (`true`).
    pub const VISIBLE: Self = Self(FfiBool::TRUE);

    /// Returns `true` if the entity is visible in at least one view.
    #[inline]
    pub const fn get(self) -> bool {
        self.0.get()
    }
}

/// Fixed-layout `#[repr(C)]` sRGBA color (`bevy_color::Srgba` / `Color::Srgba`) with `[0.0, 1.0]`
/// channels.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FfiColor {
    /// Red channel in sRGB space (`0.0..=1.0`).
    pub r: f32,
    /// Green channel in sRGB space (`0.0..=1.0`).
    pub g: f32,
    /// Blue channel in sRGB space (`0.0..=1.0`).
    pub b: f32,
    /// Alpha channel (`0.0` = transparent, `1.0` = opaque).
    pub a: f32,
}

impl_wire_abi_pod!(
    FfiColor,
    size = 16,
    align = 4,
    fields = [f32, f32, f32, f32]
);

impl FfiColor {
    /// Opaque white (`[1, 1, 1, 1]`).
    pub const WHITE: Self = Self::srgba(1.0, 1.0, 1.0, 1.0);
    /// Opaque black (`[0, 0, 0, 1]`).
    pub const BLACK: Self = Self::srgba(0.0, 0.0, 0.0, 1.0);
    /// Fully transparent black (`[0, 0, 0, 0]`).
    pub const TRANSPARENT: Self = Self::srgba(0.0, 0.0, 0.0, 0.0);
    /// Opaque red (`[1, 0, 0, 1]`).
    pub const RED: Self = Self::srgba(1.0, 0.0, 0.0, 1.0);
    /// Opaque green (`[0, 1, 0, 1]`).
    pub const GREEN: Self = Self::srgba(0.0, 1.0, 0.0, 1.0);
    /// Opaque blue (`[0, 0, 1, 1]`).
    pub const BLUE: Self = Self::srgba(0.0, 0.0, 1.0, 1.0);
    /// Opaque yellow (`[1, 1, 0, 1]`).
    pub const YELLOW: Self = Self::srgba(1.0, 1.0, 0.0, 1.0);
    /// Opaque cyan (`[0, 1, 1, 1]`).
    pub const CYAN: Self = Self::srgba(0.0, 1.0, 1.0, 1.0);
    /// Opaque magenta (`[1, 0, 1, 1]`).
    pub const MAGENTA: Self = Self::srgba(1.0, 0.0, 1.0, 1.0);

    /// Constructs an opaque sRGB color (`a = 1.0`).
    #[inline]
    pub const fn srgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Constructs an sRGBA color.
    #[inline]
    pub const fn srgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Constructs an opaque sRGB color from 8-bit channels (`0..=255`).
    #[inline]
    pub fn srgb_u8(r: u8, g: u8, b: u8) -> Self {
        Self::srgba_u8(r, g, b, 255)
    }

    /// Constructs an sRGBA color from 8-bit channels (`0..=255`).
    #[inline]
    pub fn srgba_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: f32::from(r) / 255.0,
            g: f32::from(g) / 255.0,
            b: f32::from(b) / 255.0,
            a: f32::from(a) / 255.0,
        }
    }

    /// Constructs an [`FfiColor`] by converting linear RGBA channels to sRGBA.
    #[inline]
    pub fn from_linear_rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self {
            r: linear_channel_to_srgb(r.max(0.0)),
            g: linear_channel_to_srgb(g.max(0.0)),
            b: linear_channel_to_srgb(b.max(0.0)),
            a,
        }
    }

    /// Converts this sRGBA color into linear RGBA `[r, g, b, a]`.
    #[inline]
    pub fn to_linear_rgba(self) -> [f32; 4] {
        [
            srgb_channel_to_linear(self.r.max(0.0)),
            srgb_channel_to_linear(self.g.max(0.0)),
            srgb_channel_to_linear(self.b.max(0.0)),
            self.a,
        ]
    }

    /// Returns a copy of this color with alpha replaced by `alpha`.
    #[inline]
    #[must_use]
    pub const fn with_alpha(mut self, alpha: f32) -> Self {
        self.a = alpha;
        self
    }

    /// Returns `[r, g, b, a]` as an array.
    #[inline]
    pub const fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

impl Default for FfiColor {
    #[inline]
    fn default() -> Self {
        Self::WHITE
    }
}

/// Fixed-layout `#[repr(C)]` wire projection of Bevy `Time<Virtual>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiTimeVirtual {
    /// Delta time since last advance in `f64` seconds.
    pub delta_secs_f64: f64,
    /// Total elapsed virtual time in `f64` seconds.
    pub elapsed_secs_f64: f64,
    /// Delta time since last advance in `f32` seconds.
    pub delta_secs: f32,
    /// Total elapsed virtual time in `f32` seconds.
    pub elapsed_secs: f32,
    /// Current virtual clock speed multiplier (`1.0` = real-time).
    pub relative_speed: f32,
    /// Whether the virtual clock is currently paused.
    pub paused: FfiBool,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
}

impl_wire_abi_pod!(
    FfiTimeVirtual,
    size = 32,
    align = core::mem::align_of::<u64>(),
    fields = [f64, f64, f32, f32, f32, FfiBool, [u8; 3]]
);

impl FfiTimeVirtual {
    /// Returns delta time in `f32` seconds (matching Bevy `Time::delta_secs`).
    #[inline]
    pub const fn delta_secs(&self) -> f32 {
        self.delta_secs
    }

    /// Returns delta time in `f64` seconds (matching Bevy `Time::delta_secs_f64`).
    #[inline]
    pub const fn delta_secs_f64(&self) -> f64 {
        self.delta_secs_f64
    }

    /// Returns elapsed time in `f32` seconds (matching Bevy `Time::elapsed_secs`).
    #[inline]
    pub const fn elapsed_secs(&self) -> f32 {
        self.elapsed_secs
    }

    /// Returns elapsed time in `f64` seconds (matching Bevy `Time::elapsed_secs_f64`).
    #[inline]
    pub const fn elapsed_secs_f64(&self) -> f64 {
        self.elapsed_secs_f64
    }

    /// Returns delta time as a [`Duration`] (clamped to zero if negative/non-finite).
    #[inline]
    pub fn delta(&self) -> Duration {
        Duration::try_from_secs_f64(self.delta_secs_f64).unwrap_or(Duration::ZERO)
    }

    /// Returns elapsed time as a [`Duration`] (clamped to zero if negative/non-finite).
    #[inline]
    pub fn elapsed(&self) -> Duration {
        Duration::try_from_secs_f64(self.elapsed_secs_f64).unwrap_or(Duration::ZERO)
    }

    /// Returns `true` if the virtual clock is currently paused.
    #[inline]
    pub const fn is_paused(&self) -> bool {
        self.paused.get()
    }

    /// Returns the relative virtual speed multiplier (`1.0` = real-time).
    #[inline]
    pub const fn relative_speed(&self) -> f32 {
        self.relative_speed
    }
}

/// Fixed-layout `#[repr(C)]` wire projection of Bevy `Time<Fixed>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiTimeFixed {
    /// Delta time of the current fixed tick in `f64` seconds.
    pub delta_secs_f64: f64,
    /// Total elapsed fixed time in `f64` seconds.
    pub elapsed_secs_f64: f64,
    /// Configured fixed timestep interval in `f64` seconds.
    pub timestep_secs: f64,
    /// Delta time of the current fixed tick in `f32` seconds.
    pub delta_secs: f32,
    /// Total elapsed fixed time in `f32` seconds.
    pub elapsed_secs: f32,
    /// Fractional overstep between `[0.0, 1.0)` toward the next fixed step.
    pub overstep_fraction: f32,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 4],
}

impl_wire_abi_pod!(
    FfiTimeFixed,
    size = 40,
    align = core::mem::align_of::<u64>(),
    fields = [f64, f64, f64, f32, f32, f32, [u8; 4]]
);

impl FfiTimeFixed {
    /// Returns delta time in `f32` seconds.
    #[inline]
    pub const fn delta_secs(&self) -> f32 {
        self.delta_secs
    }

    /// Returns delta time in `f64` seconds.
    #[inline]
    pub const fn delta_secs_f64(&self) -> f64 {
        self.delta_secs_f64
    }

    /// Returns elapsed time in `f32` seconds.
    #[inline]
    pub const fn elapsed_secs(&self) -> f32 {
        self.elapsed_secs
    }

    /// Returns elapsed time in `f64` seconds.
    #[inline]
    pub const fn elapsed_secs_f64(&self) -> f64 {
        self.elapsed_secs_f64
    }

    /// Returns the configured fixed timestep interval in `f64` seconds.
    #[inline]
    pub const fn timestep_secs(&self) -> f64 {
        self.timestep_secs
    }

    /// Returns the overstep fraction in `[0.0, 1.0)`.
    #[inline]
    pub const fn overstep_fraction(&self) -> f32 {
        self.overstep_fraction
    }

    /// Returns delta time as a [`Duration`].
    #[inline]
    pub fn delta(&self) -> Duration {
        Duration::try_from_secs_f64(self.delta_secs_f64).unwrap_or(Duration::ZERO)
    }

    /// Returns elapsed time as a [`Duration`].
    #[inline]
    pub fn elapsed(&self) -> Duration {
        Duration::try_from_secs_f64(self.elapsed_secs_f64).unwrap_or(Duration::ZERO)
    }

    /// Returns the configured fixed timestep as a [`Duration`].
    #[inline]
    pub fn timestep(&self) -> Duration {
        Duration::try_from_secs_f64(self.timestep_secs).unwrap_or(Duration::ZERO)
    }
}

/// Fixed-layout `#[repr(C)]` wire projection of Bevy `Time<Real>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiTimeReal {
    /// Real wall-clock delta in `f64` seconds.
    pub delta_secs_f64: f64,
    /// Total elapsed real time in `f64` seconds.
    pub elapsed_secs_f64: f64,
    /// Real wall-clock delta in `f32` seconds.
    pub delta_secs: f32,
    /// Total elapsed real time in `f32` seconds.
    pub elapsed_secs: f32,
}

impl_wire_abi_pod!(
    FfiTimeReal,
    size = 24,
    align = core::mem::align_of::<u64>(),
    fields = [f64, f64, f32, f32]
);

impl FfiTimeReal {
    /// Returns real wall-clock delta in `f32` seconds.
    #[inline]
    pub const fn delta_secs(&self) -> f32 {
        self.delta_secs
    }

    /// Returns real wall-clock delta in `f64` seconds.
    #[inline]
    pub const fn delta_secs_f64(&self) -> f64 {
        self.delta_secs_f64
    }

    /// Returns elapsed real time in `f32` seconds.
    #[inline]
    pub const fn elapsed_secs(&self) -> f32 {
        self.elapsed_secs
    }

    /// Returns elapsed real time in `f64` seconds.
    #[inline]
    pub const fn elapsed_secs_f64(&self) -> f64 {
        self.elapsed_secs_f64
    }

    /// Returns real wall-clock delta as a [`Duration`].
    #[inline]
    pub fn delta(&self) -> Duration {
        Duration::try_from_secs_f64(self.delta_secs_f64).unwrap_or(Duration::ZERO)
    }

    /// Returns elapsed real time as a [`Duration`].
    #[inline]
    pub fn elapsed(&self) -> Duration {
        Duration::try_from_secs_f64(self.elapsed_secs_f64).unwrap_or(Duration::ZERO)
    }
}

/// Compact deterministic bit-index key code (`0..194`) mirroring Bevy's `KeyCode`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FfiKeyCode(pub u16);

impl_wire_abi_pod!(FfiKeyCode, size = 2, align = 2, fields = [u16]);

#[allow(missing_docs)]
impl FfiKeyCode {
    pub const BACKQUOTE: Self = Self(0);
    pub const BACKSLASH: Self = Self(1);
    pub const BRACKET_LEFT: Self = Self(2);
    pub const BRACKET_RIGHT: Self = Self(3);
    pub const COMMA: Self = Self(4);
    pub const DIGIT_0: Self = Self(5);
    pub const DIGIT_1: Self = Self(6);
    pub const DIGIT_2: Self = Self(7);
    pub const DIGIT_3: Self = Self(8);
    pub const DIGIT_4: Self = Self(9);
    pub const DIGIT_5: Self = Self(10);
    pub const DIGIT_6: Self = Self(11);
    pub const DIGIT_7: Self = Self(12);
    pub const DIGIT_8: Self = Self(13);
    pub const DIGIT_9: Self = Self(14);
    pub const EQUAL: Self = Self(15);
    pub const KEY_A: Self = Self(19);
    pub const KEY_B: Self = Self(20);
    pub const KEY_C: Self = Self(21);
    pub const KEY_D: Self = Self(22);
    pub const KEY_E: Self = Self(23);
    pub const KEY_F: Self = Self(24);
    pub const KEY_G: Self = Self(25);
    pub const KEY_H: Self = Self(26);
    pub const KEY_I: Self = Self(27);
    pub const KEY_J: Self = Self(28);
    pub const KEY_K: Self = Self(29);
    pub const KEY_L: Self = Self(30);
    pub const KEY_M: Self = Self(31);
    pub const KEY_N: Self = Self(32);
    pub const KEY_O: Self = Self(33);
    pub const KEY_P: Self = Self(34);
    pub const KEY_Q: Self = Self(35);
    pub const KEY_R: Self = Self(36);
    pub const KEY_S: Self = Self(37);
    pub const KEY_T: Self = Self(38);
    pub const KEY_U: Self = Self(39);
    pub const KEY_V: Self = Self(40);
    pub const KEY_W: Self = Self(41);
    pub const KEY_X: Self = Self(42);
    pub const KEY_Y: Self = Self(43);
    pub const KEY_Z: Self = Self(44);
    pub const MINUS: Self = Self(45);
    pub const PERIOD: Self = Self(46);
    pub const QUOTE: Self = Self(47);
    pub const SEMICOLON: Self = Self(48);
    pub const SLASH: Self = Self(49);
    pub const ALT_LEFT: Self = Self(50);
    pub const ALT_RIGHT: Self = Self(51);
    pub const BACKSPACE: Self = Self(52);
    pub const CAPS_LOCK: Self = Self(53);
    pub const CONTROL_LEFT: Self = Self(55);
    pub const CONTROL_RIGHT: Self = Self(56);
    pub const ENTER: Self = Self(57);
    pub const SUPER_LEFT: Self = Self(58);
    pub const SUPER_RIGHT: Self = Self(59);
    pub const SHIFT_LEFT: Self = Self(60);
    pub const SHIFT_RIGHT: Self = Self(61);
    pub const SPACE: Self = Self(62);
    pub const TAB: Self = Self(63);
    pub const DELETE: Self = Self(72);
    pub const END: Self = Self(73);
    pub const HOME: Self = Self(75);
    pub const INSERT: Self = Self(76);
    pub const PAGE_DOWN: Self = Self(77);
    pub const PAGE_UP: Self = Self(78);
    pub const ARROW_DOWN: Self = Self(79);
    pub const ARROW_LEFT: Self = Self(80);
    pub const ARROW_RIGHT: Self = Self(81);
    pub const ARROW_UP: Self = Self(82);
    pub const ESCAPE: Self = Self(114);
    pub const F1: Self = Self(159);
    pub const F2: Self = Self(160);
    pub const F3: Self = Self(161);
    pub const F4: Self = Self(162);
    pub const F5: Self = Self(163);
    pub const F6: Self = Self(164);
    pub const F7: Self = Self(165);
    pub const F8: Self = Self(166);
    pub const F9: Self = Self(167);
    pub const F10: Self = Self(168);
    pub const F11: Self = Self(169);
    pub const F12: Self = Self(170);
}

/// 256-bit keyboard state snapshot (`pressed`, `just_pressed`, `just_released`) projecting
/// Bevy's `ButtonInput<KeyCode>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FfiKeyboardInput {
    /// Bitset of currently pressed keys.
    pub pressed: [u64; 4],
    /// Bitset of keys pressed during the current frame.
    pub just_pressed: [u64; 4],
    /// Bitset of keys released during the current frame.
    pub just_released: [u64; 4],
}

impl_wire_abi_pod!(
    FfiKeyboardInput,
    size = 96,
    align = core::mem::align_of::<u64>(),
    fields = [[u64; 4], [u64; 4], [u64; 4]]
);

impl FfiKeyboardInput {
    #[inline]
    const fn get_bit(words: &[u64; 4], code: FfiKeyCode) -> bool {
        let idx = code.0 as usize;
        if idx >= 256 {
            false
        } else {
            (words[idx / 64] & (1u64 << (idx % 64))) != 0
        }
    }

    #[inline]
    fn set_bit(words: &mut [u64; 4], code: FfiKeyCode, active: bool) {
        let idx = code.0 as usize;
        if idx < 256 {
            let mask = 1u64 << (idx % 64);
            if active {
                words[idx / 64] |= mask;
            } else {
                words[idx / 64] &= !mask;
            }
        }
    }

    /// Returns `true` if `code` is currently pressed.
    #[inline]
    pub const fn pressed(&self, code: FfiKeyCode) -> bool {
        Self::get_bit(&self.pressed, code)
    }

    /// Returns `true` if `code` was pressed this tick.
    #[inline]
    pub const fn just_pressed(&self, code: FfiKeyCode) -> bool {
        Self::get_bit(&self.just_pressed, code)
    }

    /// Returns `true` if `code` was released this tick.
    #[inline]
    pub const fn just_released(&self, code: FfiKeyCode) -> bool {
        Self::get_bit(&self.just_released, code)
    }

    /// Returns `true` if any key in `codes` is currently pressed.
    #[inline]
    pub fn any_pressed(&self, codes: impl IntoIterator<Item = FfiKeyCode>) -> bool {
        codes.into_iter().any(|c| self.pressed(c))
    }

    /// Returns `true` if every key in `codes` is currently pressed.
    #[inline]
    pub fn all_pressed(&self, codes: impl IntoIterator<Item = FfiKeyCode>) -> bool {
        codes.into_iter().all(|c| self.pressed(c))
    }

    /// Returns `true` if any key in `codes` was just pressed this tick.
    #[inline]
    pub fn any_just_pressed(&self, codes: impl IntoIterator<Item = FfiKeyCode>) -> bool {
        codes.into_iter().any(|c| self.just_pressed(c))
    }

    /// Returns `true` if every key in `codes` was just pressed this tick.
    #[inline]
    pub fn all_just_pressed(&self, codes: impl IntoIterator<Item = FfiKeyCode>) -> bool {
        codes.into_iter().all(|c| self.just_pressed(c))
    }

    /// Returns `true` if any key in `codes` was just released this tick.
    #[inline]
    pub fn any_just_released(&self, codes: impl IntoIterator<Item = FfiKeyCode>) -> bool {
        codes.into_iter().any(|c| self.just_released(c))
    }

    /// Returns `true` if every key in `codes` was just released this tick.
    #[inline]
    pub fn all_just_released(&self, codes: impl IntoIterator<Item = FfiKeyCode>) -> bool {
        codes.into_iter().all(|c| self.just_released(c))
    }

    /// Registers a press for `code` (sets `pressed`, and sets `just_pressed` if newly pressed).
    #[inline]
    pub fn press(&mut self, code: FfiKeyCode) {
        if !self.pressed(code) {
            self.set_just_pressed(code, true);
        }
        self.set_pressed(code, true);
    }

    /// Registers a release for `code` (clears `pressed`, and sets `just_released` if was pressed).
    #[inline]
    pub fn release(&mut self, code: FfiKeyCode) {
        if self.pressed(code) {
            self.set_just_released(code, true);
        }
        self.set_pressed(code, false);
    }

    /// Clears `just_pressed` for `code`, returning its prior value.
    #[inline]
    pub fn clear_just_pressed(&mut self, code: FfiKeyCode) -> bool {
        let was = self.just_pressed(code);
        self.set_just_pressed(code, false);
        was
    }

    /// Clears `just_released` for `code`, returning its prior value.
    #[inline]
    pub fn clear_just_released(&mut self, code: FfiKeyCode) -> bool {
        let was = self.just_released(code);
        self.set_just_released(code, false);
        was
    }

    /// Clears all `just_pressed` and `just_released` edge bits while preserving `pressed`.
    #[inline]
    pub fn clear(&mut self) {
        self.just_pressed = [0; 4];
        self.just_released = [0; 4];
    }

    /// Clears `pressed`, `just_pressed`, and `just_released` for every key.
    #[inline]
    pub fn reset_all(&mut self) {
        *self = Self::default();
    }

    /// Sets or clears the `pressed` bit for `code`.
    #[inline]
    pub fn set_pressed(&mut self, code: FfiKeyCode, active: bool) {
        Self::set_bit(&mut self.pressed, code, active);
    }

    /// Sets or clears the `just_pressed` bit for `code`.
    #[inline]
    pub fn set_just_pressed(&mut self, code: FfiKeyCode, active: bool) {
        Self::set_bit(&mut self.just_pressed, code, active);
    }

    /// Sets or clears the `just_released` bit for `code`.
    #[inline]
    pub fn set_just_released(&mut self, code: FfiKeyCode, active: bool) {
        Self::set_bit(&mut self.just_released, code, active);
    }
}

/// Compact deterministic bit-index mouse button (`0..64`) mirroring Bevy's `MouseButton`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FfiMouseButton(pub u8);

impl_wire_abi_pod!(FfiMouseButton, size = 1, align = 1, fields = [u8]);

impl FfiMouseButton {
    /// Left mouse button (`0`).
    pub const LEFT: Self = Self(0);
    /// Right mouse button (`1`).
    pub const RIGHT: Self = Self(1);
    /// Middle mouse button (`2`).
    pub const MIDDLE: Self = Self(2);
    /// Back mouse button (`3`).
    pub const BACK: Self = Self(3);
    /// Forward mouse button (`4`).
    pub const FORWARD: Self = Self(4);

    /// Constructs a mouse button handle for `MouseButton::Other(n)` (`n < 59`).
    #[inline]
    pub const fn other(n: u16) -> Option<Self> {
        let idx = 5u16.saturating_add(n);
        if idx < 64 {
            Some(Self(idx as u8))
        } else {
            None
        }
    }
}

/// 64-bit mouse button state snapshot (`pressed`, `just_pressed`, `just_released`) projecting
/// Bevy's `ButtonInput<MouseButton>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FfiMouseInput {
    /// Bitset of currently pressed mouse buttons.
    pub pressed: u64,
    /// Bitset of mouse buttons pressed during the current frame.
    pub just_pressed: u64,
    /// Bitset of mouse buttons released during the current frame.
    pub just_released: u64,
}

impl_wire_abi_pod!(
    FfiMouseInput,
    size = 24,
    align = core::mem::align_of::<u64>(),
    fields = [u64, u64, u64]
);

impl FfiMouseInput {
    #[inline]
    fn set_bit(words: &mut u64, button: FfiMouseButton, active: bool) {
        if button.0 < 64 {
            let mask = 1u64 << button.0;
            if active {
                *words |= mask;
            } else {
                *words &= !mask;
            }
        }
    }

    /// Returns `true` if `button` is currently pressed.
    #[inline]
    pub const fn pressed(&self, button: FfiMouseButton) -> bool {
        button.0 < 64 && (self.pressed & (1u64 << button.0)) != 0
    }

    /// Returns `true` if `button` was pressed this tick.
    #[inline]
    pub const fn just_pressed(&self, button: FfiMouseButton) -> bool {
        button.0 < 64 && (self.just_pressed & (1u64 << button.0)) != 0
    }

    /// Returns `true` if `button` was released this tick.
    #[inline]
    pub const fn just_released(&self, button: FfiMouseButton) -> bool {
        button.0 < 64 && (self.just_released & (1u64 << button.0)) != 0
    }

    /// Returns `true` if any button in `buttons` is currently pressed.
    #[inline]
    pub fn any_pressed(&self, buttons: impl IntoIterator<Item = FfiMouseButton>) -> bool {
        buttons.into_iter().any(|b| self.pressed(b))
    }

    /// Returns `true` if every button in `buttons` is currently pressed.
    #[inline]
    pub fn all_pressed(&self, buttons: impl IntoIterator<Item = FfiMouseButton>) -> bool {
        buttons.into_iter().all(|b| self.pressed(b))
    }

    /// Returns `true` if any button in `buttons` was just pressed this tick.
    #[inline]
    pub fn any_just_pressed(&self, buttons: impl IntoIterator<Item = FfiMouseButton>) -> bool {
        buttons.into_iter().any(|b| self.just_pressed(b))
    }

    /// Returns `true` if every button in `buttons` was just pressed this tick.
    #[inline]
    pub fn all_just_pressed(&self, buttons: impl IntoIterator<Item = FfiMouseButton>) -> bool {
        buttons.into_iter().all(|b| self.just_pressed(b))
    }

    /// Returns `true` if any button in `buttons` was just released this tick.
    #[inline]
    pub fn any_just_released(&self, buttons: impl IntoIterator<Item = FfiMouseButton>) -> bool {
        buttons.into_iter().any(|b| self.just_released(b))
    }

    /// Returns `true` if every button in `buttons` was just released this tick.
    #[inline]
    pub fn all_just_released(&self, buttons: impl IntoIterator<Item = FfiMouseButton>) -> bool {
        buttons.into_iter().all(|b| self.just_released(b))
    }

    /// Registers a press for `button`.
    #[inline]
    pub fn press(&mut self, button: FfiMouseButton) {
        if !self.pressed(button) {
            self.set_just_pressed(button, true);
        }
        self.set_pressed(button, true);
    }

    /// Registers a release for `button`.
    #[inline]
    pub fn release(&mut self, button: FfiMouseButton) {
        if self.pressed(button) {
            self.set_just_released(button, true);
        }
        self.set_pressed(button, false);
    }

    /// Clears `just_pressed` for `button`, returning its prior value.
    #[inline]
    pub fn clear_just_pressed(&mut self, button: FfiMouseButton) -> bool {
        let was = self.just_pressed(button);
        self.set_just_pressed(button, false);
        was
    }

    /// Clears `just_released` for `button`, returning its prior value.
    #[inline]
    pub fn clear_just_released(&mut self, button: FfiMouseButton) -> bool {
        let was = self.just_released(button);
        self.set_just_released(button, false);
        was
    }

    /// Clears all `just_pressed` and `just_released` edge bits while preserving `pressed`.
    #[inline]
    pub fn clear(&mut self) {
        self.just_pressed = 0;
        self.just_released = 0;
    }

    /// Clears `pressed`, `just_pressed`, and `just_released` for every button.
    #[inline]
    pub fn reset_all(&mut self) {
        *self = Self::default();
    }

    /// Sets or clears the `pressed` bit for `button`.
    #[inline]
    pub fn set_pressed(&mut self, button: FfiMouseButton, active: bool) {
        Self::set_bit(&mut self.pressed, button, active);
    }

    /// Sets or clears the `just_pressed` bit for `button`.
    #[inline]
    pub fn set_just_pressed(&mut self, button: FfiMouseButton, active: bool) {
        Self::set_bit(&mut self.just_pressed, button, active);
    }

    /// Sets or clears the `just_released` bit for `button`.
    #[inline]
    pub fn set_just_released(&mut self, button: FfiMouseButton, active: bool) {
        Self::set_bit(&mut self.just_released, button, active);
    }
}

/// Zero-padding `#[repr(C)]` POD resource projecting Bevy's `AccumulatedMouseMotion` each frame
/// (`"bevy_input::mouse::AccumulatedMouseMotion"`, 8 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiMouseMotion {
    /// Total mouse motion delta `[dx, dy]` accumulated during the current frame.
    pub delta: [f32; 2],
}

impl_wire_abi_pod!(
    FfiMouseMotion,
    size = 8,
    align = 4,
    schema = [delta: [f32; 2] => "[f32;2]"]
);

/// Zero-padding `#[repr(C)]` POD resource projecting Bevy's `AccumulatedMouseScroll` each frame
/// (`"bevy_input::mouse::AccumulatedMouseScroll"`, 12 bytes, align 4).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiMouseScroll {
    /// Total mouse scroll delta `[x, y]` accumulated during the current frame.
    pub delta: [f32; 2],
    /// Scroll unit discriminant ([`Self::UNIT_LINE`] = `0`, [`Self::UNIT_PIXEL`] = `1`).
    pub unit: u8,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
}

impl_wire_abi_pod!(
    FfiMouseScroll,
    size = 12,
    align = 4,
    schema = [
        delta: [f32; 2] => "[f32;2]",
        unit: u8 => "u8",
        reserved0: [u8; 3] => "[u8;3]",
    ]
);

impl FfiMouseScroll {
    /// Scroll delta measured in lines/rows (`MouseScrollUnit::Line` = `0`).
    pub const UNIT_LINE: u8 = 0;
    /// Scroll delta measured in pixels (`MouseScrollUnit::Pixel` = `1`).
    pub const UNIT_PIXEL: u8 = 1;

    /// Returns `true` if the scroll delta is expressed in discrete line ticks.
    #[inline]
    pub const fn is_line_unit(&self) -> bool {
        self.unit == Self::UNIT_LINE
    }

    /// Returns `true` if the scroll delta is expressed in pixels.
    #[inline]
    pub const fn is_pixel_unit(&self) -> bool {
        self.unit == Self::UNIT_PIXEL
    }
}

/// Deterministic bit-index gamepad button matching Bevy `GamepadButton` (`0..64`) in
/// `bamabi_bridges::input_ext`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FfiGamepadButton(pub u8);

impl_wire_abi_pod!(FfiGamepadButton, size = 1, align = 1, fields = [u8]);

#[allow(missing_docs)]
impl FfiGamepadButton {
    pub const SOUTH: Self = Self(0);
    pub const EAST: Self = Self(1);
    pub const NORTH: Self = Self(2);
    pub const WEST: Self = Self(3);
    pub const C: Self = Self(4);
    pub const Z: Self = Self(5);
    pub const LEFT_TRIGGER: Self = Self(6);
    pub const LEFT_TRIGGER2: Self = Self(7);
    pub const RIGHT_TRIGGER: Self = Self(8);
    pub const RIGHT_TRIGGER2: Self = Self(9);
    pub const SELECT: Self = Self(10);
    pub const START: Self = Self(11);
    pub const MODE: Self = Self(12);
    pub const LEFT_THUMB: Self = Self(13);
    pub const RIGHT_THUMB: Self = Self(14);
    pub const DPAD_UP: Self = Self(15);
    pub const DPAD_DOWN: Self = Self(16);
    pub const DPAD_LEFT: Self = Self(17);
    pub const DPAD_RIGHT: Self = Self(18);
    /// Starting bit index for vendor-specific `GamepadButton::Other(u8)` buttons (`19..64`).
    pub const OTHER_BASE: u8 = 19;

    /// Constructs an `Other(index)` gamepad button (`0..45`), returning `None` if out of range.
    #[inline]
    pub const fn other(index: u8) -> Option<Self> {
        let bit = Self::OTHER_BASE.saturating_add(index);
        if bit < 64 { Some(Self(bit)) } else { None }
    }

    /// Returns the 64-bit bitmask for this button (`0` if `>= 64`).
    #[inline]
    pub const fn bit_mask(self) -> u64 {
        if self.0 < 64 { 1u64 << self.0 } else { 0 }
    }
}

/// Zero-padding `#[repr(C)]` POD component projecting Bevy `0.20`'s `Gamepad` entity component
/// (`"bevy_input::gamepad::Gamepad"`, 72 bytes, align 8 on 64-bit / 4 on i686).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGamepadState {
    /// 64-bit bitmask of currently pressed [`FfiGamepadButton`]s.
    pub pressed: u64,
    /// 64-bit bitmask of [`FfiGamepadButton`]s pressed during the current frame.
    pub just_pressed: u64,
    /// 64-bit bitmask of [`FfiGamepadButton`]s released during the current frame.
    pub just_released: u64,
    /// Left analog stick `[x, y]` in `[-1.0, 1.0]`.
    pub left_stick: [f32; 2],
    /// Right analog stick `[x, y]` in `[-1.0, 1.0]`.
    pub right_stick: [f32; 2],
    /// Left analog trigger (`GamepadButton::LeftTrigger2`) in `[0.0, 1.0]`.
    pub left_trigger: f32,
    /// Right analog trigger (`GamepadButton::RightTrigger2`) in `[0.0, 1.0]`.
    pub right_trigger: f32,
    /// Left Z axis (`GamepadAxis::LeftZ`) in `[-1.0, 1.0]`.
    pub left_z: f32,
    /// Right Z axis (`GamepadAxis::RightZ`) in `[-1.0, 1.0]`.
    pub right_z: f32,
    /// Directional pad vector `[x, y]` in `[-1.0, 1.0]`.
    pub dpad: [f32; 2],
    /// USB vendor ID when [`Self::has_vendor_id`] is true.
    pub vendor_id: u16,
    /// USB product ID when [`Self::has_product_id`] is true.
    pub product_id: u16,
    /// Whether [`Self::vendor_id`] is populated.
    pub has_vendor_id: FfiBool,
    /// Whether [`Self::product_id`] is populated.
    pub has_product_id: FfiBool,
    /// Whether the underlying `Gamepad` component is connected/active.
    pub connected: FfiBool,
    /// Explicit zero-reserved alignment byte.
    pub reserved0: u8,
}

impl_wire_abi_pod!(
    FfiGamepadState,
    size = 72,
    align = core::mem::align_of::<u64>(),
    schema = [
        pressed: u64 => "u64",
        just_pressed: u64 => "u64",
        just_released: u64 => "u64",
        left_stick: [f32; 2] => "[f32;2]",
        right_stick: [f32; 2] => "[f32;2]",
        left_trigger: f32 => "f32",
        right_trigger: f32 => "f32",
        left_z: f32 => "f32",
        right_z: f32 => "f32",
        dpad: [f32; 2] => "[f32;2]",
        vendor_id: u16 => "u16",
        product_id: u16 => "u16",
        has_vendor_id: FfiBool => "FfiBool",
        has_product_id: FfiBool => "FfiBool",
        connected: FfiBool => "FfiBool",
        reserved0: u8 => "u8",
    ]
);

impl FfiGamepadState {
    /// Returns `true` if `button` is currently pressed.
    #[inline]
    pub const fn pressed(&self, button: FfiGamepadButton) -> bool {
        (self.pressed & button.bit_mask()) != 0
    }

    /// Returns `true` if `button` was just pressed this tick.
    #[inline]
    pub const fn just_pressed(&self, button: FfiGamepadButton) -> bool {
        (self.just_pressed & button.bit_mask()) != 0
    }

    /// Returns `true` if `button` was just released this tick.
    #[inline]
    pub const fn just_released(&self, button: FfiGamepadButton) -> bool {
        (self.just_released & button.bit_mask()) != 0
    }

    /// Returns `true` if any button in `buttons` is currently pressed.
    #[inline]
    pub fn any_pressed(&self, buttons: impl IntoIterator<Item = FfiGamepadButton>) -> bool {
        buttons.into_iter().any(|b| self.pressed(b))
    }

    /// Returns the USB vendor ID when populated.
    #[inline]
    pub const fn vendor_id_opt(&self) -> Option<u16> {
        if self.has_vendor_id.get() {
            Some(self.vendor_id)
        } else {
            None
        }
    }

    /// Returns the USB product ID when populated.
    #[inline]
    pub const fn product_id_opt(&self) -> Option<u16> {
        if self.has_product_id.get() {
            Some(self.product_id)
        } else {
            None
        }
    }

    /// Returns `true` if this gamepad is connected.
    #[inline]
    pub const fn is_connected(&self) -> bool {
        self.connected.get()
    }
}

/// Zero-padding `#[repr(C)]` haptic rumble message mirroring Bevy `GamepadRumbleRequest`
/// (`"bevy_input::gamepad::GamepadRumbleRequest"`, 24 bytes, align 8 on 64-bit / 4 on i686).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FfiGamepadRumble {
    /// Target `Gamepad` entity (`FfiEntity::NULL` is ignored).
    pub gamepad: bamabi_sys::FfiEntity,
    /// Rumble duration in seconds for [`Self::KIND_ADD`] (must be finite and `>= 0.0`).
    pub duration_secs: f32,
    /// Low-frequency strong motor intensity in `[0.0, 1.0]` (clamped on translation).
    pub strong_motor: f32,
    /// High-frequency weak motor intensity in `[0.0, 1.0]` (clamped on translation).
    pub weak_motor: f32,
    /// Request kind: [`Self::KIND_ADD`] (`0`) or [`Self::KIND_STOP`] (`1`).
    pub kind: u8,
    /// Explicit zero-reserved alignment bytes.
    pub reserved0: [u8; 3],
}

impl_wire_abi_pod!(
    FfiGamepadRumble,
    size = 24,
    align = core::mem::align_of::<bamabi_sys::FfiEntity>(),
    schema = [
        gamepad: bamabi_sys::FfiEntity => "FfiEntity",
        duration_secs: f32 => "f32",
        strong_motor: f32 => "f32",
        weak_motor: f32 => "f32",
        kind: u8 => "u8",
        reserved0: [u8; 3] => "[u8;3]",
    ]
);

impl FfiGamepadRumble {
    /// Enqueue/add a rumble effect (`GamepadRumbleRequest::Add` = `0`).
    pub const KIND_ADD: u8 = 0;
    /// Stop all active rumble effects (`GamepadRumbleRequest::Stop` = `1`).
    pub const KIND_STOP: u8 = 1;

    /// Constructs a rumble `Add` request for `gamepad`.
    #[inline]
    pub const fn add(
        gamepad: bamabi_sys::FfiEntity,
        duration_secs: f32,
        strong_motor: f32,
        weak_motor: f32,
    ) -> Self {
        Self {
            gamepad,
            duration_secs,
            strong_motor,
            weak_motor,
            kind: Self::KIND_ADD,
            reserved0: [0; 3],
        }
    }

    /// Constructs a rumble `Stop` request for `gamepad`.
    #[inline]
    pub const fn stop(gamepad: bamabi_sys::FfiEntity) -> Self {
        Self {
            gamepad,
            duration_secs: 0.0,
            strong_motor: 0.0,
            weak_motor: 0.0,
            kind: Self::KIND_STOP,
            reserved0: [0; 3],
        }
    }
}
