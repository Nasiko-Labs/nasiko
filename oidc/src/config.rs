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
    /// Space-separated scopes, e.g. `"openid profile email"`.
    pub scopes: String,
}
