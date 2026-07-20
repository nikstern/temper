//! Cedar policy generation from a multi-dimensional scope matrix.
//!
//! Replaces the old Narrow/Medium/Broad enum with a composable matrix of
//! principal × action × resource × duration scopes. Each dimension is
//! independently selectable, giving fine-grained control over generated Cedar
//! policies.

use serde::{Deserialize, Serialize};

/// Who the policy applies to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalScope {
    /// Only the specific agent that was denied.
    ThisAgent,
    /// All agents sharing a particular role.
    AgentsWithRole,
    /// All agents of a specific type (e.g. "claude-code").
    AgentsOfType,
    /// Any authenticated agent.
    AnyAgent,
}

/// Which actions the policy covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionScope {
    /// Only the specific denied action.
    ThisAction,
    /// All actions on the specified resource type.
    AllActionsOnType,
    /// All actions on any resource.
    AllActions,
}

/// Which resources the policy covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScope {
    /// Only the exact resource that was denied.
    ThisResource,
    /// Any resource of the same type.
    AnyOfType,
    /// Any resource of any type.
    AnyResource,
}

/// How long the policy lasts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DurationScope {
    /// Scoped to a specific session (adds sessionId condition).
    Session,
    /// Permanent policy.
    Always,
}

/// Multi-dimensional policy scope matrix.
///
/// Each dimension is independently selectable. The matrix is serialized as JSON
/// and stored on approved `PendingDecision` records.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyScopeMatrix {
    /// Who the policy applies to.
    pub principal: PrincipalScope,
    /// Which actions are covered.
    pub action: ActionScope,
    /// Which resources are covered.
    pub resource: ResourceScope,
    /// How long the policy lasts.
    pub duration: DurationScope,
    /// Required when `principal == AgentsOfType`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type_value: Option<String>,
    /// Required when `principal == AgentsWithRole`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_value: Option<String>,
    /// Required when `duration == Session`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl PolicyScopeMatrix {
    /// Sensible default: ThisAgent + ThisAction + AnyOfType + Always.
    ///
    /// Equivalent to the old "medium" scope. If `agent_type` is provided,
    /// stores it for potential use with `AgentsOfType`.
    pub fn default_for(agent_type: Option<&str>) -> Self {
        Self {
            principal: PrincipalScope::ThisAgent,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Always,
            agent_type_value: agent_type.map(String::from),
            role_value: None,
            session_id: None,
        }
    }
}

/// Validate that a scope matrix is internally consistent.
///
/// Returns an error if required companion fields are missing or empty:
/// - `principal = AgentsOfType` requires non-empty `agent_type_value`
/// - `principal = AgentsWithRole` requires non-empty `role_value`
/// - `duration = Session` requires non-empty `session_id`
pub fn validate_policy_scope_matrix(matrix: &PolicyScopeMatrix) -> Result<(), String> {
    if matrix.principal == PrincipalScope::AgentsOfType {
        let Some(agent_type) = matrix.agent_type_value.as_deref() else {
            return Err("principal=agents_of_type requires agent_type_value".to_string());
        };
        if agent_type.trim().is_empty() {
            return Err("principal=agents_of_type requires non-empty agent_type_value".to_string());
        }
    }

    if matrix.principal == PrincipalScope::AgentsWithRole {
        let Some(role) = matrix.role_value.as_deref() else {
            return Err("principal=agents_with_role requires role_value".to_string());
        };
        if role.trim().is_empty() {
            return Err("principal=agents_with_role requires non-empty role_value".to_string());
        }
    }

    if matrix.duration == DurationScope::Session {
        let Some(session_id) = matrix.session_id.as_deref() else {
            return Err("duration=session requires session_id".to_string());
        };
        if session_id.trim().is_empty() {
            return Err("duration=session requires non-empty session_id".to_string());
        }
    }

    Ok(())
}

/// Generate a Cedar permit statement from a scope matrix.
///
/// Each matrix dimension maps to a specific Cedar clause:
/// - **PrincipalScope**: principal clause
/// - **ActionScope**: action clause
/// - **ResourceScope**: resource clause
/// - **DurationScope**: optional `when` condition for session scoping
pub fn generate_cedar_from_matrix(
    agent_id: &str,
    principal_kind: &str,
    action: &str,
    resource_type: &str,
    resource_id: &str,
    matrix: &PolicyScopeMatrix,
) -> String {
    // Pre-assertions (TigerStyle): companion fields must be present when their scope requires them.
    debug_assert!(
        matrix.principal != PrincipalScope::AgentsOfType || matrix.agent_type_value.is_some(),
        "AgentsOfType requires agent_type_value"
    );
    debug_assert!(
        matrix.principal != PrincipalScope::AgentsWithRole || matrix.role_value.is_some(),
        "AgentsWithRole requires role_value"
    );
    debug_assert!(
        matrix.duration != DurationScope::Session || matrix.session_id.is_some(),
        "Session duration requires session_id"
    );

    let principal_clause = match &matrix.principal {
        PrincipalScope::ThisAgent => {
            format!("principal == {}::\"{}\"", principal_kind, agent_id)
        }
        PrincipalScope::AgentsWithRole
        | PrincipalScope::AgentsOfType
        | PrincipalScope::AnyAgent => format!("principal is {}", principal_kind),
    };

    let action_clause = match &matrix.action {
        ActionScope::ThisAction => format!("action == Action::\"{}\"", action),
        ActionScope::AllActionsOnType | ActionScope::AllActions => "action".to_string(),
    };

    let resource_clause = match &matrix.resource {
        ResourceScope::ThisResource => {
            format!("resource == {}::\"{}\"", resource_type, resource_id)
        }
        ResourceScope::AnyOfType => format!("resource is {}", resource_type),
        ResourceScope::AnyResource => "resource".to_string(),
    };

    // Build when conditions.
    let mut conditions: Vec<String> = Vec::new();

    match &matrix.principal {
        PrincipalScope::AgentsWithRole => {
            if let Some(ref role) = matrix.role_value {
                conditions.push(format!("context.role == \"{}\"", role));
            }
        }
        PrincipalScope::AgentsOfType => {
            if let Some(ref agent_type) = matrix.agent_type_value {
                conditions.push(format!("context.agentType == \"{}\"", agent_type));
                // Require credential-verified identity (ADR-0033).
                conditions.push("context.agentTypeVerified == true".to_string());
            }
        }
        _ => {}
    }

    if matrix.duration == DurationScope::Session
        && let Some(ref session_id) = matrix.session_id
    {
        conditions.push(format!("context.sessionId == \"{}\"", session_id));
    }

    let when_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("\nwhen {{ {} }}", conditions.join(" && "))
    };

    format!(
        "permit(\n  {},\n  {},\n  {}\n){};",
        principal_clause, action_clause, resource_clause, when_clause,
    )
}

// --- Spec-declared authorization (RFC-0002, ARN-255) ------------------------

/// One action's declared authorization requirements, extracted from its IOA
/// spec. Kept as primitives so this module needs no dependency on temper-spec.
#[derive(Debug, Clone)]
pub struct ActionAuthz {
    /// Action name (e.g. "Publish").
    pub name: String,
    /// Roles allowed to invoke it (`requires_role`); empty = no role gate.
    pub requires_role: Vec<String>,
    /// `"creator"` restricts to the resource owner; None = no ownership gate.
    pub requires: Option<String>,
}

/// Escape a string for use inside a Cedar double-quoted string literal.
///
/// App-authored identifiers (action names, role values) flow into generated
/// policy text; without escaping a `"` or `\` would break the policy or inject
/// clauses (the ARN-172 class). Cedar string escapes are backslash-based.
fn cedar_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            _ => out.push(c),
        }
    }
    out
}

/// A Cedar entity-type name is an unquoted identifier, not a string literal, so
/// it cannot be escaped — it must already be a valid identifier. Reject
/// anything else rather than emit broken/injectable policy text.
fn is_valid_cedar_ident(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Compile an entity's action authorization annotations into Cedar
/// `forbid … unless` overlays.
///
/// A **forbid** overlay is used (not a permit) so the requirement composes on
/// top of an app's existing broad `permit(principal, action, resource is X)`
/// base: Cedar's forbid-overrides-permit semantics mean the overlay restricts
/// even when a blanket permit exists. Both requirements on one action emit two
/// forbids (role AND ownership must both hold to invoke).
///
/// Returns `""` when no action carries a requirement, or when the entity type
/// is not a valid Cedar identifier (defensive — never emit broken policy text).
pub fn generate_authz_overlays(entity_type: &str, actions: &[ActionAuthz]) -> String {
    if !is_valid_cedar_ident(entity_type) {
        return String::new();
    }
    let mut out = String::new();
    for a in actions {
        let action_lit = cedar_escape(&a.name);
        // The comment sits after `//`; strip line breaks so a newline in an
        // app-authored name cannot break out of the comment and inject a clause.
        let comment_name = comment_safe(&a.name);

        if !a.requires_role.is_empty() {
            let roles = a
                .requires_role
                .iter()
                .map(|r| format!("\"{}\"", cedar_escape(r)))
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!(
                "// generated from spec: {entity_type}.{comment_name} requires_role\n\
                 forbid(\n  principal,\n  action == Action::\"{action_lit}\",\n  resource is {entity_type}\n)\nunless {{ principal has role && [{roles}].contains(principal.role) }};\n\n",
            ));
        }

        if a.requires.as_deref() == Some("creator") {
            out.push_str(&format!(
                "// generated from spec: {entity_type}.{comment_name} requires creator\n\
                 forbid(\n  principal,\n  action == Action::\"{action_lit}\",\n  resource is {entity_type}\n)\nunless {{ resource has creator_sub && resource.creator_sub == principal.id }};\n\n",
            ));
        }
    }
    out
}

/// Collapse line breaks to spaces so a value is safe inside a `//` comment.
fn comment_safe(s: &str) -> String {
    s.replace(['\n', '\r'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_this_agent_this_action_this_resource() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::ThisAgent,
            action: ActionScope::ThisAction,
            resource: ResourceScope::ThisResource,
            duration: DurationScope::Always,
            agent_type_value: None,
            role_value: None,
            session_id: None,
        };
        let policy =
            generate_cedar_from_matrix("bot-1", "Agent", "submitOrder", "Order", "order-123", &m);
        assert!(policy.contains("principal == Agent::\"bot-1\""));
        assert!(policy.contains("action == Action::\"submitOrder\""));
        assert!(policy.contains("resource == Order::\"order-123\""));
        assert!(!policy.contains("when"));
    }

    #[test]
    fn test_this_agent_this_action_any_of_type() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::ThisAgent,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Always,
            agent_type_value: None,
            role_value: None,
            session_id: None,
        };
        let policy =
            generate_cedar_from_matrix("bot-1", "Agent", "submitOrder", "Order", "order-123", &m);
        assert!(policy.contains("resource is Order"));
    }

    #[test]
    fn test_any_agent_all_actions_any_resource() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::AnyAgent,
            action: ActionScope::AllActions,
            resource: ResourceScope::AnyResource,
            duration: DurationScope::Always,
            agent_type_value: None,
            role_value: None,
            session_id: None,
        };
        let policy =
            generate_cedar_from_matrix("bot-1", "Agent", "submitOrder", "Order", "order-123", &m);
        assert!(policy.contains("principal is Agent"));
        assert!(!policy.contains("Action::"));
        assert!(!policy.contains("Order"));
    }

    #[test]
    fn test_agents_of_type_condition() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::AgentsOfType,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Always,
            agent_type_value: Some("claude-code".to_string()),
            role_value: None,
            session_id: None,
        };
        let policy =
            generate_cedar_from_matrix("bot-1", "Agent", "submitOrder", "Order", "order-123", &m);
        assert!(policy.contains("principal is Agent"));
        assert!(policy.contains("context.agentType == \"claude-code\""));
        assert!(policy.contains("context.agentTypeVerified == true"));
    }

    #[test]
    fn test_agents_with_role_condition() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::AgentsWithRole,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Always,
            agent_type_value: None,
            role_value: Some("operations_agent".to_string()),
            session_id: None,
        };
        let policy =
            generate_cedar_from_matrix("bot-1", "Agent", "submitOrder", "Order", "order-123", &m);
        assert!(policy.contains("context.role == \"operations_agent\""));
    }

    #[test]
    fn test_session_duration_adds_session_id() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::ThisAgent,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Session,
            agent_type_value: None,
            role_value: None,
            session_id: Some("sess-abc".to_string()),
        };
        let policy =
            generate_cedar_from_matrix("bot-1", "Agent", "submitOrder", "Order", "order-123", &m);
        assert!(policy.contains("context.sessionId == \"sess-abc\""));
    }

    #[test]
    fn test_combined_agent_type_and_session() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::AgentsOfType,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Session,
            agent_type_value: Some("openclaw".to_string()),
            role_value: None,
            session_id: Some("sess-xyz".to_string()),
        };
        let policy =
            generate_cedar_from_matrix("bot-1", "Agent", "submitOrder", "Order", "order-123", &m);
        assert!(policy.contains("context.agentType == \"openclaw\""));
        assert!(policy.contains("context.sessionId == \"sess-xyz\""));
    }

    #[test]
    fn test_all_actions_on_type_still_constrains_resource() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::ThisAgent,
            action: ActionScope::AllActionsOnType,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Always,
            agent_type_value: None,
            role_value: None,
            session_id: None,
        };
        let policy =
            generate_cedar_from_matrix("bot-1", "Agent", "submitOrder", "Order", "order-123", &m);
        assert!(policy.contains("resource is Order"));
        assert!(!policy.contains("Action::"));
    }

    #[test]
    fn test_default_matrix() {
        let m = PolicyScopeMatrix::default_for(Some("claude-code"));
        assert_eq!(m.principal, PrincipalScope::ThisAgent);
        assert_eq!(m.action, ActionScope::ThisAction);
        assert_eq!(m.resource, ResourceScope::AnyOfType);
        assert_eq!(m.duration, DurationScope::Always);
        assert_eq!(m.agent_type_value, Some("claude-code".to_string()));
    }

    #[test]
    fn validate_matrix_requires_agent_type_for_agents_of_type() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::AgentsOfType,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Always,
            agent_type_value: None,
            role_value: None,
            session_id: None,
        };
        assert!(validate_policy_scope_matrix(&m).is_err());
    }

    #[test]
    fn validate_matrix_requires_role_for_agents_with_role() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::AgentsWithRole,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Always,
            agent_type_value: None,
            role_value: None,
            session_id: None,
        };
        assert!(validate_policy_scope_matrix(&m).is_err());
    }

    #[test]
    fn validate_matrix_requires_session_for_session_duration() {
        let m = PolicyScopeMatrix {
            principal: PrincipalScope::ThisAgent,
            action: ActionScope::ThisAction,
            resource: ResourceScope::AnyOfType,
            duration: DurationScope::Session,
            agent_type_value: None,
            role_value: None,
            session_id: None,
        };
        assert!(validate_policy_scope_matrix(&m).is_err());
    }

    // --- Spec-declared authorization overlays -------------------------------

    fn authz(name: &str, roles: &[&str], requires: Option<&str>) -> ActionAuthz {
        ActionAuthz {
            name: name.to_string(),
            requires_role: roles.iter().map(|s| s.to_string()).collect(),
            requires: requires.map(|s| s.to_string()),
        }
    }

    #[test]
    fn no_annotations_emits_nothing() {
        let out = generate_authz_overlays("DesignLanguage", &[authz("Publish", &[], None)]);
        assert!(out.is_empty());
    }

    #[test]
    fn requires_role_emits_forbid_unless_role() {
        let out = generate_authz_overlays(
            "DesignLanguage",
            &[authz("Publish", &["owner", "curator"], None)],
        );
        assert!(out.contains("forbid("));
        assert!(out.contains("action == Action::\"Publish\""));
        assert!(out.contains("resource is DesignLanguage"));
        assert!(out.contains("principal has role"));
        assert!(out.contains("[\"owner\", \"curator\"].contains(principal.role)"));
        // It is a forbid overlay, never a permit.
        assert!(!out.contains("permit("));
    }

    #[test]
    fn requires_creator_emits_ownership_forbid() {
        let out = generate_authz_overlays("Remix", &[authz("Withdraw", &[], Some("creator"))]);
        assert!(out.contains("resource has creator_sub && resource.creator_sub == principal.id"));
        assert!(out.contains("action == Action::\"Withdraw\""));
    }

    #[test]
    fn both_requirements_emit_two_forbids() {
        let out = generate_authz_overlays("Doc", &[authz("Edit", &["owner"], Some("creator"))]);
        assert_eq!(out.matches("forbid(").count(), 2);
    }

    #[test]
    fn identifiers_are_escaped_against_injection() {
        // A malicious action name trying to close the string and inject a permit.
        let evil = "X\") };\npermit(principal, action, resource);\n//";
        let out = generate_authz_overlays("Doc", &[authz(evil, &["owner"], None)]);
        // The injected permit text must not appear as a live clause — the quote
        // is escaped, so it stays inside the Action string literal.
        assert!(!out.contains("\npermit(principal, action, resource);"));
        assert!(out.contains("\\\""));
    }

    #[test]
    fn invalid_entity_type_emits_nothing() {
        let out = generate_authz_overlays("Bad Name", &[authz("Publish", &["owner"], None)]);
        assert!(out.is_empty());
    }
}
