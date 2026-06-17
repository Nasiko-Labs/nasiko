// TODO: Refactor from current gateway/ crate.
// Gateway architecture:
// - Pingora HTTP proxy as public-facing listener
// - Auth via dyn AuthProvider (OSS: SingleUserAuth)
// - Rate limiting (IP + user)
// - CORS handling
// - Static UI file serving (embedded via rust-embed)
// - Proxy /api/* to backend server
// - A2A agent proxying (routes /agents/{id}/* to agent endpoints)
// - A2A discovery endpoint (/.well-known/, /a2a/)
// - Flow limits enforcement (traceparent depth, fan-out, tokens, timeout)

fn main() {
    todo!("Gateway implementation — refactor from current gateway/ crate")
}
