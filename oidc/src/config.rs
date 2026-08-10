/// Static configuration for one OIDC relying party. Nothing here is specific
/// to Microsoft Entra ID — `issuer_url` is simply whichever OIDC-compliant
/// authority is configured (Entra, Okta, Auth0, Keycloak, Google...).
#[derive(Debug, Clone)]
pub struct OidcConfig {
    /// The issuer authority, e.g. `https://login.microsoftonline.com/<tenant-id>/v2.0`.
    /// Discovery is fetched from `{issuer_url}/.well-known/openid-configuration`.
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    /// Must exactly match the redirect URI registered with the IdP.
    pub redirect_uri: String,
    /// Fleet-relay callback override. When set — a multi-tenant workspace CP
    /// behind the shared fleet OAuth app — BOTH the authorization request and the
    /// token exchange use this instead of [`Self::redirect_uri`] (OIDC requires
    /// the two to match). It is the ONE fixed callback registered with the IdP,
    /// e.g. `https://nasiko.dev/auth/oidc/callback` — with NO tenant-id path
    /// suffix, because Google exact-matches redirect_uri. The BFF's OIDC relay
    /// dispatches to the right CP by the tenant id the CP carries in the OAuth
    /// `state` (unlike GitHub's `central_callback_url`, which is path-based).
    pub central_callback_url: Option<String>,
    /// Space-separated scopes, e.g. `"openid profile email"`.
    pub scopes: String,
}
