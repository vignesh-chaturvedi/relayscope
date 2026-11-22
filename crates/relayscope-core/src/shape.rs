//! Each Borsh type records the layout it was written for, so a test can compare it with the
//! pinned IDL field by field. If the program changes a type, that test fails instead of the
//! decoder silently reading the wrong bytes.

/// The layout of one IDL type, as written in Rust.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub name: &'static str,
    pub kind: ShapeKind,
}

#[derive(Clone, Copy, Debug)]
pub enum ShapeKind {
    /// Field names and their Rust types, in Borsh order.
    Struct(&'static [(&'static str, &'static str)]),
    /// Variant names, in tag order.
    Enum(&'static [&'static str]),
}

/// A type whose Borsh layout mirrors an IDL type.
pub trait IdlType {
    const SHAPE: Shape;
}

/// Defines a Borsh struct and records its layout for the IDL test.
macro_rules! idl_struct {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $( $(#[$fmeta:meta])* pub $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(
            Clone, Debug, PartialEq, Eq,
            borsh::BorshSerialize, borsh::BorshDeserialize, serde::Serialize,
        )]
        pub struct $name {
            $( $(#[$fmeta])* pub $field: $ty ),*
        }

        impl $crate::shape::IdlType for $name {
            const SHAPE: $crate::shape::Shape = $crate::shape::Shape {
                name: stringify!($name),
                kind: $crate::shape::ShapeKind::Struct(&[$((stringify!($field), stringify!($ty))),*]),
            };
        }
    };
}

/// Defines a fieldless Borsh enum and records its variants for the IDL test.
macro_rules! idl_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident { $( $(#[$vmeta:meta])* $variant:ident ),* $(,)? }
    ) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash,
            borsh::BorshSerialize, borsh::BorshDeserialize, serde::Serialize,
        )]
        #[borsh(use_discriminant = false)]
        pub enum $name {
            $( $(#[$vmeta])* $variant ),*
        }

        impl $crate::shape::IdlType for $name {
            const SHAPE: $crate::shape::Shape = $crate::shape::Shape {
                name: stringify!($name),
                kind: $crate::shape::ShapeKind::Enum(&[$(stringify!($variant)),*]),
            };
        }
    };
}

pub(crate) use idl_enum;
pub(crate) use idl_struct;
