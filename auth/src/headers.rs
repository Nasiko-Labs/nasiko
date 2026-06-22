/// Header names used by the gateway to forward authenticated identity to the server.
/// Defined here (in nasiko-auth) so both gateway and server use the same constants
/// without a circular dependency.

pub const HEADER_USER_ID: &str = "x-user-id";
pub const HEADER_USERNAME: &str = "x-username";
pub const HEADER_IS_SUPERUSER: &str = "x-is-superuser";
pub const HEADER_USER_ROLE: &str = "x-user-role";
/// EE only in practice — set when the user has a team assignment.
pub const HEADER_TEAM_ID: &str = "x-user-team-id";
/// EE only in practice — set when the user has a department assignment.
pub const HEADER_DEPT_ID: &str = "x-user-dept-id";

/// All trust headers that must be stripped from incoming client requests
/// before the gateway runs auth, to prevent spoofing.
pub const TRUST_HEADERS: &[&str] = &[
    HEADER_USER_ID,
    HEADER_USERNAME,
    HEADER_IS_SUPERUSER,
    HEADER_USER_ROLE,
    HEADER_TEAM_ID,
    HEADER_DEPT_ID,
];
