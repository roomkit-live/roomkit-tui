//! Opaque identities. Their text is assigned by the host or client and never
//! interpreted by the protocol.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! identifier {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(
            Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Wrap an opaque identity.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// The identity's text.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Take the identity's text.
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

identifier!(
    ConversationId,
    "An opaque conversation identity assigned by the host."
);
identifier!(
    ActorId,
    "An opaque participant identity, distinct from a display name."
);
identifier!(MessageId, "A message identity within one conversation.");
identifier!(
    OperationId,
    "A client operation identity retained when retrying across connections."
);
identifier!(RunId, "An execution identity within one conversation.");
identifier!(CallId, "A tool call identity within one execution.");
identifier!(
    PermissionId,
    "An outstanding human decision identity within a conversation."
);
identifier!(
    QuestionId,
    "An outstanding question to the person, as the host names it."
);

/// A client-generated correlation identifier. It is local to one connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(u64);

impl RequestId {
    /// Wrap a connection-local correlation number.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The correlation number.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for RequestId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
