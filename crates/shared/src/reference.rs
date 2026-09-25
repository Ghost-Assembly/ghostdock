//! The API and MCP reference: what the server offers, generated from the
//! code that offers it, so it cannot drift from what is mounted.

use serde::{Deserialize, Serialize};

use crate::token::Permission;

/// Everything `GET /api/v1/reference` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub endpoints: Vec<Endpoint>,
    pub tools: Vec<Tool>,
    pub permissions: Vec<PermissionInfo>,
}

/// One method on one path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    /// `GET`, `POST`, `PUT` or `DELETE`.
    pub method: String,
    /// The full path, with parameters in braces: `/api/v1/stacks/{id}`.
    pub path: String,
    /// The heading it is listed under.
    pub area: String,
    pub access: Access,
    /// What it does, in a sentence.
    pub summary: String,
}

/// Who may call an endpoint.
///
/// On the wire: `{"kind":"token","permission":"host.view"}`, or just
/// `{"kind":"session"}` for the kinds that name no permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "permission", rename_all = "snake_case")]
pub enum Access {
    /// Anyone, signed in or not.
    Public,
    /// A person signed in through the browser. Tokens are refused.
    Session,
    /// A signed-in person, or any valid token whatever it was granted.
    Authenticated,
    /// A signed-in person, or a token granted this permission.
    Token(Permission),
    /// A token only; what it can then do depends on its permissions.
    AnyToken,
}

impl Access {
    /// The permission it names, if any.
    #[must_use]
    pub fn permission(self) -> Option<Permission> {
        match self {
            Self::Token(p) => Some(p),
            _ => None,
        }
    }

    /// A short label for a person reading the reference.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Public => "Public",
            Self::Session => "Signed-in person only",
            Self::Authenticated => "Signed in, or any token",
            Self::Token(p) => p.as_str(),
            Self::AnyToken => "Any token",
        }
    }
}

/// One MCP tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub title: String,
    pub description: String,
    /// A token needs this for the tool to be listed or called.
    pub permission: Permission,
    /// The tool's input as JSON Schema, as JSON text.
    pub input_schema: String,
    pub read_only: bool,
    pub destructive: bool,
}

/// One permission a token can be granted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionInfo {
    pub permission: Permission,
    pub area: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_names_its_kind_and_only_a_token_names_a_permission() {
        assert_eq!(
            serde_json::to_value(Access::Token(Permission::HostView)).unwrap(),
            serde_json::json!({ "kind": "token", "permission": "host.view" })
        );
        assert_eq!(
            serde_json::to_value(Access::Session).unwrap(),
            serde_json::json!({ "kind": "session" })
        );
        for access in [
            Access::Public,
            Access::Session,
            Access::Authenticated,
            Access::Token(Permission::ShellOpen),
            Access::AnyToken,
        ] {
            let text = serde_json::to_string(&access).unwrap();
            assert_eq!(serde_json::from_str::<Access>(&text).unwrap(), access);
        }
    }
}
