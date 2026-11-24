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

/// Compact deterministic bit-index key code (`0..194`) mirroring every named Bevy `0.20` `KeyCode`.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FfiKeyCode(pub u16);

impl_wire_abi_pod!(FfiKeyCode, size = 2, align = 2, fields = [u16]);

impl FfiKeyCode {
    /// Total number of named Bevy `0.20` `KeyCode` indices (`0..194`).
    pub const COUNT: usize = 194;

    /// Bevy `KeyCode` index `0` (`BACKQUOTE`).
    pub const BACKQUOTE: Self = Self(0);
    /// Bevy `KeyCode` index `1` (`BACKSLASH`).
    pub const BACKSLASH: Self = Self(1);
    /// Bevy `KeyCode` index `2` (`BRACKET_LEFT`).
    pub const BRACKET_LEFT: Self = Self(2);
    /// Bevy `KeyCode` index `3` (`BRACKET_RIGHT`).
    pub const BRACKET_RIGHT: Self = Self(3);
    /// Bevy `KeyCode` index `4` (`COMMA`).
    pub const COMMA: Self = Self(4);
    /// Bevy `KeyCode` index `5` (`DIGIT_0`).
    pub const DIGIT_0: Self = Self(5);
    /// Bevy `KeyCode` index `6` (`DIGIT_1`).
    pub const DIGIT_1: Self = Self(6);
    /// Bevy `KeyCode` index `7` (`DIGIT_2`).
    pub const DIGIT_2: Self = Self(7);
    /// Bevy `KeyCode` index `8` (`DIGIT_3`).
    pub const DIGIT_3: Self = Self(8);
    /// Bevy `KeyCode` index `9` (`DIGIT_4`).
    pub const DIGIT_4: Self = Self(9);
    /// Bevy `KeyCode` index `10` (`DIGIT_5`).
    pub const DIGIT_5: Self = Self(10);
    /// Bevy `KeyCode` index `11` (`DIGIT_6`).
    pub const DIGIT_6: Self = Self(11);
    /// Bevy `KeyCode` index `12` (`DIGIT_7`).
    pub const DIGIT_7: Self = Self(12);
    /// Bevy `KeyCode` index `13` (`DIGIT_8`).
    pub const DIGIT_8: Self = Self(13);
    /// Bevy `KeyCode` index `14` (`DIGIT_9`).
    pub const DIGIT_9: Self = Self(14);
    /// Bevy `KeyCode` index `15` (`EQUAL`).
    pub const EQUAL: Self = Self(15);
    /// Bevy `KeyCode` index `16` (`INTL_BACKSLASH`).
    pub const INTL_BACKSLASH: Self = Self(16);
    /// Bevy `KeyCode` index `17` (`INTL_RO`).
    pub const INTL_RO: Self = Self(17);
    /// Bevy `KeyCode` index `18` (`INTL_YEN`).
    pub const INTL_YEN: Self = Self(18);
    /// Bevy `KeyCode` index `19` (`KEY_A`).
    pub const KEY_A: Self = Self(19);
    /// Bevy `KeyCode` index `20` (`KEY_B`).
    pub const KEY_B: Self = Self(20);
    /// Bevy `KeyCode` index `21` (`KEY_C`).
    pub const KEY_C: Self = Self(21);
    /// Bevy `KeyCode` index `22` (`KEY_D`).
    pub const KEY_D: Self = Self(22);
    /// Bevy `KeyCode` index `23` (`KEY_E`).
    pub const KEY_E: Self = Self(23);
    /// Bevy `KeyCode` index `24` (`KEY_F`).
    pub const KEY_F: Self = Self(24);
    /// Bevy `KeyCode` index `25` (`KEY_G`).
    pub const KEY_G: Self = Self(25);
    /// Bevy `KeyCode` index `26` (`KEY_H`).
    pub const KEY_H: Self = Self(26);
    /// Bevy `KeyCode` index `27` (`KEY_I`).
    pub const KEY_I: Self = Self(27);
    /// Bevy `KeyCode` index `28` (`KEY_J`).
    pub const KEY_J: Self = Self(28);
    /// Bevy `KeyCode` index `29` (`KEY_K`).
    pub const KEY_K: Self = Self(29);
    /// Bevy `KeyCode` index `30` (`KEY_L`).
    pub const KEY_L: Self = Self(30);
    /// Bevy `KeyCode` index `31` (`KEY_M`).
    pub const KEY_M: Self = Self(31);
    /// Bevy `KeyCode` index `32` (`KEY_N`).
    pub const KEY_N: Self = Self(32);
    /// Bevy `KeyCode` index `33` (`KEY_O`).
    pub const KEY_O: Self = Self(33);
    /// Bevy `KeyCode` index `34` (`KEY_P`).
    pub const KEY_P: Self = Self(34);
    /// Bevy `KeyCode` index `35` (`KEY_Q`).
    pub const KEY_Q: Self = Self(35);
    /// Bevy `KeyCode` index `36` (`KEY_R`).
    pub const KEY_R: Self = Self(36);
    /// Bevy `KeyCode` index `37` (`KEY_S`).
    pub const KEY_S: Self = Self(37);
    /// Bevy `KeyCode` index `38` (`KEY_T`).
    pub const KEY_T: Self = Self(38);
    /// Bevy `KeyCode` index `39` (`KEY_U`).
    pub const KEY_U: Self = Self(39);
    /// Bevy `KeyCode` index `40` (`KEY_V`).
    pub const KEY_V: Self = Self(40);
    /// Bevy `KeyCode` index `41` (`KEY_W`).
    pub const KEY_W: Self = Self(41);
    /// Bevy `KeyCode` index `42` (`KEY_X`).
    pub const KEY_X: Self = Self(42);
    /// Bevy `KeyCode` index `43` (`KEY_Y`).
    pub const KEY_Y: Self = Self(43);
    /// Bevy `KeyCode` index `44` (`KEY_Z`).
    pub const KEY_Z: Self = Self(44);
    /// Bevy `KeyCode` index `45` (`MINUS`).
    pub const MINUS: Self = Self(45);
    /// Bevy `KeyCode` index `46` (`PERIOD`).
    pub const PERIOD: Self = Self(46);
    /// Bevy `KeyCode` index `47` (`QUOTE`).
    pub const QUOTE: Self = Self(47);
    /// Bevy `KeyCode` index `48` (`SEMICOLON`).
    pub const SEMICOLON: Self = Self(48);
    /// Bevy `KeyCode` index `49` (`SLASH`).
    pub const SLASH: Self = Self(49);
    /// Bevy `KeyCode` index `50` (`ALT_LEFT`).
    pub const ALT_LEFT: Self = Self(50);
    /// Bevy `KeyCode` index `51` (`ALT_RIGHT`).
    pub const ALT_RIGHT: Self = Self(51);
    /// Bevy `KeyCode` index `52` (`BACKSPACE`).
    pub const BACKSPACE: Self = Self(52);
    /// Bevy `KeyCode` index `53` (`CAPS_LOCK`).
    pub const CAPS_LOCK: Self = Self(53);
    /// Bevy `KeyCode` index `54` (`CONTEXT_MENU`).
    pub const CONTEXT_MENU: Self = Self(54);
    /// Bevy `KeyCode` index `55` (`CONTROL_LEFT`).
    pub const CONTROL_LEFT: Self = Self(55);
    /// Bevy `KeyCode` index `56` (`CONTROL_RIGHT`).
    pub const CONTROL_RIGHT: Self = Self(56);
    /// Bevy `KeyCode` index `57` (`ENTER`).
    pub const ENTER: Self = Self(57);
    /// Bevy `KeyCode` index `58` (`SUPER_LEFT`).
    pub const SUPER_LEFT: Self = Self(58);
    /// Bevy `KeyCode` index `59` (`SUPER_RIGHT`).
    pub const SUPER_RIGHT: Self = Self(59);
    /// Bevy `KeyCode` index `60` (`SHIFT_LEFT`).
    pub const SHIFT_LEFT: Self = Self(60);
    /// Bevy `KeyCode` index `61` (`SHIFT_RIGHT`).
    pub const SHIFT_RIGHT: Self = Self(61);
    /// Bevy `KeyCode` index `62` (`SPACE`).
    pub const SPACE: Self = Self(62);
    /// Bevy `KeyCode` index `63` (`TAB`).
    pub const TAB: Self = Self(63);
    /// Bevy `KeyCode` index `64` (`CONVERT`).
    pub const CONVERT: Self = Self(64);
    /// Bevy `KeyCode` index `65` (`KANA_MODE`).
    pub const KANA_MODE: Self = Self(65);
    /// Bevy `KeyCode` index `66` (`LANG_1`).
    pub const LANG_1: Self = Self(66);
    /// Bevy `KeyCode` index `67` (`LANG_2`).
    pub const LANG_2: Self = Self(67);
    /// Bevy `KeyCode` index `68` (`LANG_3`).
    pub const LANG_3: Self = Self(68);
    /// Bevy `KeyCode` index `69` (`LANG_4`).
    pub const LANG_4: Self = Self(69);
    /// Bevy `KeyCode` index `70` (`LANG_5`).
    pub const LANG_5: Self = Self(70);
    /// Bevy `KeyCode` index `71` (`NON_CONVERT`).
    pub const NON_CONVERT: Self = Self(71);
    /// Bevy `KeyCode` index `72` (`DELETE`).
    pub const DELETE: Self = Self(72);
    /// Bevy `KeyCode` index `73` (`END`).
    pub const END: Self = Self(73);
    /// Bevy `KeyCode` index `74` (`HELP`).
    pub const HELP: Self = Self(74);
    /// Bevy `KeyCode` index `75` (`HOME`).
    pub const HOME: Self = Self(75);
    /// Bevy `KeyCode` index `76` (`INSERT`).
    pub const INSERT: Self = Self(76);
    /// Bevy `KeyCode` index `77` (`PAGE_DOWN`).
    pub const PAGE_DOWN: Self = Self(77);
    /// Bevy `KeyCode` index `78` (`PAGE_UP`).
    pub const PAGE_UP: Self = Self(78);
    /// Bevy `KeyCode` index `79` (`ARROW_DOWN`).
    pub const ARROW_DOWN: Self = Self(79);
    /// Bevy `KeyCode` index `80` (`ARROW_LEFT`).
    pub const ARROW_LEFT: Self = Self(80);
    /// Bevy `KeyCode` index `81` (`ARROW_RIGHT`).
    pub const ARROW_RIGHT: Self = Self(81);
    /// Bevy `KeyCode` index `82` (`ARROW_UP`).
    pub const ARROW_UP: Self = Self(82);
    /// Bevy `KeyCode` index `83` (`NUM_LOCK`).
    pub const NUM_LOCK: Self = Self(83);
    /// Bevy `KeyCode` index `84` (`NUMPAD_0`).
    pub const NUMPAD_0: Self = Self(84);
    /// Bevy `KeyCode` index `85` (`NUMPAD_1`).
    pub const NUMPAD_1: Self = Self(85);
    /// Bevy `KeyCode` index `86` (`NUMPAD_2`).
    pub const NUMPAD_2: Self = Self(86);
    /// Bevy `KeyCode` index `87` (`NUMPAD_3`).
    pub const NUMPAD_3: Self = Self(87);
    /// Bevy `KeyCode` index `88` (`NUMPAD_4`).
    pub const NUMPAD_4: Self = Self(88);
    /// Bevy `KeyCode` index `89` (`NUMPAD_5`).
    pub const NUMPAD_5: Self = Self(89);
    /// Bevy `KeyCode` index `90` (`NUMPAD_6`).
    pub const NUMPAD_6: Self = Self(90);
    /// Bevy `KeyCode` index `91` (`NUMPAD_7`).
    pub const NUMPAD_7: Self = Self(91);
    /// Bevy `KeyCode` index `92` (`NUMPAD_8`).
    pub const NUMPAD_8: Self = Self(92);
    /// Bevy `KeyCode` index `93` (`NUMPAD_9`).
    pub const NUMPAD_9: Self = Self(93);
    /// Bevy `KeyCode` index `94` (`NUMPAD_ADD`).
    pub const NUMPAD_ADD: Self = Self(94);
    /// Bevy `KeyCode` index `95` (`NUMPAD_BACKSPACE`).
    pub const NUMPAD_BACKSPACE: Self = Self(95);
    /// Bevy `KeyCode` index `96` (`NUMPAD_CLEAR`).
    pub const NUMPAD_CLEAR: Self = Self(96);
    /// Bevy `KeyCode` index `97` (`NUMPAD_CLEAR_ENTRY`).
    pub const NUMPAD_CLEAR_ENTRY: Self = Self(97);
    /// Bevy `KeyCode` index `98` (`NUMPAD_COMMA`).
    pub const NUMPAD_COMMA: Self = Self(98);
    /// Bevy `KeyCode` index `99` (`NUMPAD_DECIMAL`).
    pub const NUMPAD_DECIMAL: Self = Self(99);
    /// Bevy `KeyCode` index `100` (`NUMPAD_DIVIDE`).
    pub const NUMPAD_DIVIDE: Self = Self(100);
    /// Bevy `KeyCode` index `101` (`NUMPAD_ENTER`).
    pub const NUMPAD_ENTER: Self = Self(101);
    /// Bevy `KeyCode` index `102` (`NUMPAD_EQUAL`).
    pub const NUMPAD_EQUAL: Self = Self(102);
    /// Bevy `KeyCode` index `103` (`NUMPAD_HASH`).
    pub const NUMPAD_HASH: Self = Self(103);
    /// Bevy `KeyCode` index `104` (`NUMPAD_MEMORY_ADD`).
    pub const NUMPAD_MEMORY_ADD: Self = Self(104);
    /// Bevy `KeyCode` index `105` (`NUMPAD_MEMORY_CLEAR`).
    pub const NUMPAD_MEMORY_CLEAR: Self = Self(105);
    /// Bevy `KeyCode` index `106` (`NUMPAD_MEMORY_RECALL`).
    pub const NUMPAD_MEMORY_RECALL: Self = Self(106);
    /// Bevy `KeyCode` index `107` (`NUMPAD_MEMORY_STORE`).
    pub const NUMPAD_MEMORY_STORE: Self = Self(107);
    /// Bevy `KeyCode` index `108` (`NUMPAD_MEMORY_SUBTRACT`).
    pub const NUMPAD_MEMORY_SUBTRACT: Self = Self(108);
    /// Bevy `KeyCode` index `109` (`NUMPAD_MULTIPLY`).
    pub const NUMPAD_MULTIPLY: Self = Self(109);
    /// Bevy `KeyCode` index `110` (`NUMPAD_PAREN_LEFT`).
    pub const NUMPAD_PAREN_LEFT: Self = Self(110);
    /// Bevy `KeyCode` index `111` (`NUMPAD_PAREN_RIGHT`).
    pub const NUMPAD_PAREN_RIGHT: Self = Self(111);
    /// Bevy `KeyCode` index `112` (`NUMPAD_STAR`).
    pub const NUMPAD_STAR: Self = Self(112);
    /// Bevy `KeyCode` index `113` (`NUMPAD_SUBTRACT`).
    pub const NUMPAD_SUBTRACT: Self = Self(113);
    /// Bevy `KeyCode` index `114` (`ESCAPE`).
    pub const ESCAPE: Self = Self(114);
    /// Bevy `KeyCode` index `115` (`FN`).
    pub const FN: Self = Self(115);
    /// Bevy `KeyCode` index `116` (`FN_LOCK`).
    pub const FN_LOCK: Self = Self(116);
    /// Bevy `KeyCode` index `117` (`PRINT_SCREEN`).
    pub const PRINT_SCREEN: Self = Self(117);
    /// Bevy `KeyCode` index `118` (`SCROLL_LOCK`).
    pub const SCROLL_LOCK: Self = Self(118);
    /// Bevy `KeyCode` index `119` (`PAUSE`).
    pub const PAUSE: Self = Self(119);
    /// Bevy `KeyCode` index `120` (`BROWSER_BACK`).
    pub const BROWSER_BACK: Self = Self(120);
    /// Bevy `KeyCode` index `121` (`BROWSER_FAVORITES`).
    pub const BROWSER_FAVORITES: Self = Self(121);
    /// Bevy `KeyCode` index `122` (`BROWSER_FORWARD`).
    pub const BROWSER_FORWARD: Self = Self(122);
    /// Bevy `KeyCode` index `123` (`BROWSER_HOME`).
    pub const BROWSER_HOME: Self = Self(123);
    /// Bevy `KeyCode` index `124` (`BROWSER_REFRESH`).
    pub const BROWSER_REFRESH: Self = Self(124);
    /// Bevy `KeyCode` index `125` (`BROWSER_SEARCH`).
    pub const BROWSER_SEARCH: Self = Self(125);
    /// Bevy `KeyCode` index `126` (`BROWSER_STOP`).
    pub const BROWSER_STOP: Self = Self(126);
    /// Bevy `KeyCode` index `127` (`EJECT`).
    pub const EJECT: Self = Self(127);
    /// Bevy `KeyCode` index `128` (`LAUNCH_APP_1`).
    pub const LAUNCH_APP_1: Self = Self(128);
    /// Bevy `KeyCode` index `129` (`LAUNCH_APP_2`).
    pub const LAUNCH_APP_2: Self = Self(129);
    /// Bevy `KeyCode` index `130` (`LAUNCH_MAIL`).
    pub const LAUNCH_MAIL: Self = Self(130);
    /// Bevy `KeyCode` index `131` (`MEDIA_PLAY_PAUSE`).
    pub const MEDIA_PLAY_PAUSE: Self = Self(131);
    /// Bevy `KeyCode` index `132` (`MEDIA_SELECT`).
    pub const MEDIA_SELECT: Self = Self(132);
    /// Bevy `KeyCode` index `133` (`MEDIA_STOP`).
    pub const MEDIA_STOP: Self = Self(133);
    /// Bevy `KeyCode` index `134` (`MEDIA_TRACK_NEXT`).
    pub const MEDIA_TRACK_NEXT: Self = Self(134);
    /// Bevy `KeyCode` index `135` (`MEDIA_TRACK_PREVIOUS`).
    pub const MEDIA_TRACK_PREVIOUS: Self = Self(135);
    /// Bevy `KeyCode` index `136` (`POWER`).
    pub const POWER: Self = Self(136);
    /// Bevy `KeyCode` index `137` (`SLEEP`).
    pub const SLEEP: Self = Self(137);
    /// Bevy `KeyCode` index `138` (`AUDIO_VOLUME_DOWN`).
    pub const AUDIO_VOLUME_DOWN: Self = Self(138);
    /// Bevy `KeyCode` index `139` (`AUDIO_VOLUME_MUTE`).
    pub const AUDIO_VOLUME_MUTE: Self = Self(139);
    /// Bevy `KeyCode` index `140` (`AUDIO_VOLUME_UP`).
    pub const AUDIO_VOLUME_UP: Self = Self(140);
    /// Bevy `KeyCode` index `141` (`WAKE_UP`).
    pub const WAKE_UP: Self = Self(141);
    /// Bevy `KeyCode` index `142` (`META`).
    pub const META: Self = Self(142);
    /// Bevy `KeyCode` index `143` (`HYPER`).
    pub const HYPER: Self = Self(143);
    /// Bevy `KeyCode` index `144` (`TURBO`).
    pub const TURBO: Self = Self(144);
    /// Bevy `KeyCode` index `145` (`ABORT`).
    pub const ABORT: Self = Self(145);
    /// Bevy `KeyCode` index `146` (`RESUME`).
    pub const RESUME: Self = Self(146);
    /// Bevy `KeyCode` index `147` (`SUSPEND`).
    pub const SUSPEND: Self = Self(147);
    /// Bevy `KeyCode` index `148` (`AGAIN`).
    pub const AGAIN: Self = Self(148);
    /// Bevy `KeyCode` index `149` (`COPY`).
    pub const COPY: Self = Self(149);
    /// Bevy `KeyCode` index `150` (`CUT`).
    pub const CUT: Self = Self(150);
    /// Bevy `KeyCode` index `151` (`FIND`).
    pub const FIND: Self = Self(151);
    /// Bevy `KeyCode` index `152` (`OPEN`).
    pub const OPEN: Self = Self(152);
    /// Bevy `KeyCode` index `153` (`PASTE`).
    pub const PASTE: Self = Self(153);
    /// Bevy `KeyCode` index `154` (`PROPS`).
    pub const PROPS: Self = Self(154);
    /// Bevy `KeyCode` index `155` (`SELECT`).
    pub const SELECT: Self = Self(155);
    /// Bevy `KeyCode` index `156` (`UNDO`).
    pub const UNDO: Self = Self(156);
    /// Bevy `KeyCode` index `157` (`HIRAGANA`).
    pub const HIRAGANA: Self = Self(157);
    /// Bevy `KeyCode` index `158` (`KATAKANA`).
    pub const KATAKANA: Self = Self(158);
    /// Bevy `KeyCode` index `159` (`F1`).
    pub const F1: Self = Self(159);
    /// Bevy `KeyCode` index `160` (`F2`).
    pub const F2: Self = Self(160);
    /// Bevy `KeyCode` index `161` (`F3`).
    pub const F3: Self = Self(161);
    /// Bevy `KeyCode` index `162` (`F4`).
    pub const F4: Self = Self(162);
    /// Bevy `KeyCode` index `163` (`F5`).
    pub const F5: Self = Self(163);
    /// Bevy `KeyCode` index `164` (`F6`).
    pub const F6: Self = Self(164);
    /// Bevy `KeyCode` index `165` (`F7`).
    pub const F7: Self = Self(165);
    /// Bevy `KeyCode` index `166` (`F8`).
    pub const F8: Self = Self(166);
    /// Bevy `KeyCode` index `167` (`F9`).
    pub const F9: Self = Self(167);
    /// Bevy `KeyCode` index `168` (`F10`).
    pub const F10: Self = Self(168);
    /// Bevy `KeyCode` index `169` (`F11`).
    pub const F11: Self = Self(169);
    /// Bevy `KeyCode` index `170` (`F12`).
    pub const F12: Self = Self(170);
    /// Bevy `KeyCode` index `171` (`F13`).
    pub const F13: Self = Self(171);
    /// Bevy `KeyCode` index `172` (`F14`).
    pub const F14: Self = Self(172);
    /// Bevy `KeyCode` index `173` (`F15`).
    pub const F15: Self = Self(173);
    /// Bevy `KeyCode` index `174` (`F16`).
    pub const F16: Self = Self(174);
    /// Bevy `KeyCode` index `175` (`F17`).
    pub const F17: Self = Self(175);
    /// Bevy `KeyCode` index `176` (`F18`).
    pub const F18: Self = Self(176);
    /// Bevy `KeyCode` index `177` (`F19`).
    pub const F19: Self = Self(177);
    /// Bevy `KeyCode` index `178` (`F20`).
    pub const F20: Self = Self(178);
    /// Bevy `KeyCode` index `179` (`F21`).
    pub const F21: Self = Self(179);
    /// Bevy `KeyCode` index `180` (`F22`).
    pub const F22: Self = Self(180);
    /// Bevy `KeyCode` index `181` (`F23`).
    pub const F23: Self = Self(181);
    /// Bevy `KeyCode` index `182` (`F24`).
    pub const F24: Self = Self(182);
    /// Bevy `KeyCode` index `183` (`F25`).
    pub const F25: Self = Self(183);
    /// Bevy `KeyCode` index `184` (`F26`).
    pub const F26: Self = Self(184);
    /// Bevy `KeyCode` index `185` (`F27`).
    pub const F27: Self = Self(185);
    /// Bevy `KeyCode` index `186` (`F28`).
    pub const F28: Self = Self(186);
    /// Bevy `KeyCode` index `187` (`F29`).
    pub const F29: Self = Self(187);
    /// Bevy `KeyCode` index `188` (`F30`).
    pub const F30: Self = Self(188);
    /// Bevy `KeyCode` index `189` (`F31`).
    pub const F31: Self = Self(189);
    /// Bevy `KeyCode` index `190` (`F32`).
    pub const F32: Self = Self(190);
    /// Bevy `KeyCode` index `191` (`F33`).
    pub const F33: Self = Self(191);
    /// Bevy `KeyCode` index `192` (`F34`).
    pub const F34: Self = Self(192);
    /// Bevy `KeyCode` index `193` (`F35`).
    pub const F35: Self = Self(193);

    /// Complete ordered table of all 194 named Bevy `0.20` `KeyCode` indices (`0..=193`).
    pub const ALL: [Self; Self::COUNT] = [
        Self::BACKQUOTE,
        Self::BACKSLASH,
        Self::BRACKET_LEFT,
        Self::BRACKET_RIGHT,
        Self::COMMA,
        Self::DIGIT_0,
        Self::DIGIT_1,
        Self::DIGIT_2,
        Self::DIGIT_3,
        Self::DIGIT_4,
        Self::DIGIT_5,
        Self::DIGIT_6,
        Self::DIGIT_7,
        Self::DIGIT_8,
        Self::DIGIT_9,
        Self::EQUAL,
        Self::INTL_BACKSLASH,
        Self::INTL_RO,
        Self::INTL_YEN,
        Self::KEY_A,
        Self::KEY_B,
        Self::KEY_C,
        Self::KEY_D,
        Self::KEY_E,
        Self::KEY_F,
        Self::KEY_G,
        Self::KEY_H,
        Self::KEY_I,
        Self::KEY_J,
        Self::KEY_K,
        Self::KEY_L,
        Self::KEY_M,
        Self::KEY_N,
        Self::KEY_O,
        Self::KEY_P,
        Self::KEY_Q,
        Self::KEY_R,
        Self::KEY_S,
        Self::KEY_T,
        Self::KEY_U,
        Self::KEY_V,
        Self::KEY_W,
        Self::KEY_X,
        Self::KEY_Y,
        Self::KEY_Z,
        Self::MINUS,
        Self::PERIOD,
        Self::QUOTE,
        Self::SEMICOLON,
        Self::SLASH,
        Self::ALT_LEFT,
        Self::ALT_RIGHT,
        Self::BACKSPACE,
        Self::CAPS_LOCK,
        Self::CONTEXT_MENU,
        Self::CONTROL_LEFT,
        Self::CONTROL_RIGHT,
        Self::ENTER,
        Self::SUPER_LEFT,
        Self::SUPER_RIGHT,
        Self::SHIFT_LEFT,
        Self::SHIFT_RIGHT,
        Self::SPACE,
        Self::TAB,
        Self::CONVERT,
        Self::KANA_MODE,
        Self::LANG_1,
        Self::LANG_2,
        Self::LANG_3,
        Self::LANG_4,
        Self::LANG_5,
        Self::NON_CONVERT,
        Self::DELETE,
        Self::END,
        Self::HELP,
        Self::HOME,
        Self::INSERT,
        Self::PAGE_DOWN,
        Self::PAGE_UP,
        Self::ARROW_DOWN,
        Self::ARROW_LEFT,
        Self::ARROW_RIGHT,
        Self::ARROW_UP,
        Self::NUM_LOCK,
        Self::NUMPAD_0,
        Self::NUMPAD_1,
        Self::NUMPAD_2,
        Self::NUMPAD_3,
        Self::NUMPAD_4,
        Self::NUMPAD_5,
        Self::NUMPAD_6,
        Self::NUMPAD_7,
        Self::NUMPAD_8,
        Self::NUMPAD_9,
        Self::NUMPAD_ADD,
        Self::NUMPAD_BACKSPACE,
        Self::NUMPAD_CLEAR,
        Self::NUMPAD_CLEAR_ENTRY,
        Self::NUMPAD_COMMA,
        Self::NUMPAD_DECIMAL,
        Self::NUMPAD_DIVIDE,
        Self::NUMPAD_ENTER,
        Self::NUMPAD_EQUAL,
        Self::NUMPAD_HASH,
        Self::NUMPAD_MEMORY_ADD,
        Self::NUMPAD_MEMORY_CLEAR,
        Self::NUMPAD_MEMORY_RECALL,
        Self::NUMPAD_MEMORY_STORE,
        Self::NUMPAD_MEMORY_SUBTRACT,
        Self::NUMPAD_MULTIPLY,
        Self::NUMPAD_PAREN_LEFT,
        Self::NUMPAD_PAREN_RIGHT,
        Self::NUMPAD_STAR,
        Self::NUMPAD_SUBTRACT,
        Self::ESCAPE,
        Self::FN,
        Self::FN_LOCK,
        Self::PRINT_SCREEN,
        Self::SCROLL_LOCK,
        Self::PAUSE,
        Self::BROWSER_BACK,
        Self::BROWSER_FAVORITES,
        Self::BROWSER_FORWARD,
        Self::BROWSER_HOME,
        Self::BROWSER_REFRESH,
        Self::BROWSER_SEARCH,
        Self::BROWSER_STOP,
        Self::EJECT,
        Self::LAUNCH_APP_1,
        Self::LAUNCH_APP_2,
        Self::LAUNCH_MAIL,
        Self::MEDIA_PLAY_PAUSE,
        Self::MEDIA_SELECT,
        Self::MEDIA_STOP,
        Self::MEDIA_TRACK_NEXT,
        Self::MEDIA_TRACK_PREVIOUS,
        Self::POWER,
        Self::SLEEP,
        Self::AUDIO_VOLUME_DOWN,
        Self::AUDIO_VOLUME_MUTE,
        Self::AUDIO_VOLUME_UP,
        Self::WAKE_UP,
        Self::META,
        Self::HYPER,
        Self::TURBO,
        Self::ABORT,
        Self::RESUME,
        Self::SUSPEND,
        Self::AGAIN,
        Self::COPY,
        Self::CUT,
        Self::FIND,
        Self::OPEN,
        Self::PASTE,
        Self::PROPS,
        Self::SELECT,
        Self::UNDO,
        Self::HIRAGANA,
        Self::KATAKANA,
        Self::F1,
        Self::F2,
        Self::F3,
        Self::F4,
        Self::F5,
        Self::F6,
        Self::F7,
        Self::F8,
        Self::F9,
        Self::F10,
        Self::F11,
        Self::F12,
        Self::F13,
        Self::F14,
        Self::F15,
        Self::F16,
        Self::F17,
        Self::F18,
        Self::F19,
        Self::F20,
        Self::F21,
        Self::F22,
        Self::F23,
        Self::F24,
        Self::F25,
        Self::F26,
        Self::F27,
        Self::F28,
        Self::F29,
        Self::F30,
        Self::F31,
        Self::F32,
        Self::F33,
        Self::F34,
        Self::F35,
    ];

    /// Returns `true` if this key code lies within the named Bevy `0.20` key table (`0..194`).
    #[inline]
    pub const fn is_valid(self) -> bool {
        (self.0 as usize) < Self::COUNT
    }
}

const _: () = {
    let mut i = 0;
    while i < FfiKeyCode::COUNT {
        assert!(FfiKeyCode::ALL[i].0 as usize == i);
        i += 1;
    }
};

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

impl FfiGamepadButton {
    /// Bottom face button (`GamepadButton::South` / A / Cross).
    pub const SOUTH: Self = Self(0);
    /// Right face button (`GamepadButton::East` / B / Circle).
    pub const EAST: Self = Self(1);
    /// Top face button (`GamepadButton::North` / Y / Triangle).
    pub const NORTH: Self = Self(2);
    /// Left face button (`GamepadButton::West` / X / Square).
    pub const WEST: Self = Self(3);
    /// C button (`GamepadButton::C`).
    pub const C: Self = Self(4);
    /// Z button (`GamepadButton::Z`).
    pub const Z: Self = Self(5);
    /// Left shoulder bumper (`GamepadButton::LeftTrigger`).
    pub const LEFT_TRIGGER: Self = Self(6);
    /// Left analog/digital trigger (`GamepadButton::LeftTrigger2`).
    pub const LEFT_TRIGGER2: Self = Self(7);
    /// Right shoulder bumper (`GamepadButton::RightTrigger`).
    pub const RIGHT_TRIGGER: Self = Self(8);
    /// Right analog/digital trigger (`GamepadButton::RightTrigger2`).
    pub const RIGHT_TRIGGER2: Self = Self(9);
    /// Select / Back / Share button (`GamepadButton::Select`).
    pub const SELECT: Self = Self(10);
    /// Start / Options / Menu button (`GamepadButton::Start`).
    pub const START: Self = Self(11);
    /// Home / Guide / Mode button (`GamepadButton::Mode`).
    pub const MODE: Self = Self(12);
    /// Left stick click (`GamepadButton::LeftThumb`).
    pub const LEFT_THUMB: Self = Self(13);
    /// Right stick click (`GamepadButton::RightThumb`).
    pub const RIGHT_THUMB: Self = Self(14);
    /// Directional pad up (`GamepadButton::DPadUp`).
    pub const DPAD_UP: Self = Self(15);
    /// Directional pad down (`GamepadButton::DPadDown`).
    pub const DPAD_DOWN: Self = Self(16);
    /// Directional pad left (`GamepadButton::DPadLeft`).
    pub const DPAD_LEFT: Self = Self(17);
    /// Directional pad right (`GamepadButton::DPadRight`).
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

    /// Alias for [`Self::bit_mask`] matching `bamabi_bridges::input_ext::FfiGamepadButton::bit`.
    #[inline]
    pub const fn bit(self) -> u64 {
        self.bit_mask()
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
