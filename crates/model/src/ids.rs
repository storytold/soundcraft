//! Typed ids. All ids come from one session-wide counter so they never collide across kinds.

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(TrackId);
id_type!(ClipId);
id_type!(ClipDefinitionId);
id_type!(SourceId);
id_type!(BusId);
id_type!(GroupId);
id_type!(MarkerId);
