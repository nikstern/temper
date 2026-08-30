use super::ActionParam;

impl ActionParam {
    /// Return the logical parameter name.
    pub fn name(&self) -> &str {
        match self {
            Self::Named(name) => name,
            Self::Typed { name, .. } => name,
        }
    }

    /// Return the declared parameter type, defaulting plain names to `string`.
    pub fn param_type(&self) -> &str {
        match self {
            Self::Named(_) => "string",
            Self::Typed { param_type, .. } => param_type,
        }
    }

    /// Return the declared target entity type for a typed reference parameter.
    pub fn entity_type(&self) -> Option<&str> {
        match self {
            Self::Named(_) => None,
            Self::Typed { entity_type, .. } => entity_type.as_deref(),
        }
    }

    /// Return whether the parameter explicitly permits absence or JSON `null`.
    pub fn nullable(&self) -> bool {
        match self {
            Self::Named(_) => false,
            Self::Typed { nullable, .. } => *nullable,
        }
    }
}
