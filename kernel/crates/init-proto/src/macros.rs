//! `open_enum!`: a string enum that keeps unknown values as `Other(String)`.
//!
//! Used for value sets a newer peer may extend (job states, stage names, instance kinds), so an
//! old client still parses the frame. Closed sets use plain `#[derive(Serialize, Deserialize)]`.

macro_rules! open_enum {
    (
        $(#[$m:meta])*
        pub enum $name:ident { $( $(#[$vm:meta])* $var:ident = $s:literal, )* }
    ) => {
        $(#[$m])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$vm])* $var, )*
            /// A value this build does not know (kept verbatim).
            Other(String),
        }

        impl $name {
            /// Every known value.
            pub const ALL: &'static [$name] = &[ $( $name::$var, )* ];

            /// The wire string.
            pub fn as_str(&self) -> &str {
                match self {
                    $( $name::$var => $s, )*
                    $name::Other(s) => s,
                }
            }

            /// Parse a wire string; unknown values become `Other`.
            pub fn parse(s: &str) -> $name {
                match s {
                    $( $s => $name::$var, )*
                    other => $name::Other(other.to_string()),
                }
            }

            /// True for a value this build knows.
            pub fn is_known(&self) -> bool {
                !matches!(self, $name::Other(_))
            }
        }

        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl ::serde::Serialize for $name {
            fn serialize<S: ::serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.as_str())
            }
        }

        impl<'de> ::serde::Deserialize<'de> for $name {
            fn deserialize<D: ::serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = <String as ::serde::Deserialize>::deserialize(d)?;
                Ok($name::parse(&s))
            }
        }
    };
}
