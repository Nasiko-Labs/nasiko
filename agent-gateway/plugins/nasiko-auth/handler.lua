local http = require "resty.http"
local json = require "cjson"

local NasikoAuthHandler = {
  VERSION = "1.0.0",
  PRIORITY = 1000,
}

local function validate_token(auth_service_url, token, timeout)
  local httpc = http.new()
  httpc:set_timeout(timeout)

  local res, err = httpc:request_uri(auth_service_url .. "/auth/validate", {
    method = "POST",
    headers = {
      ["Content-Type"] = "application/json",
      ["Authorization"] = "Bearer " .. token
    },
    body = json.encode({
      token = token
    })
  })

  if not res then
    kong.log.err("Failed to call auth service: ", err)
    return false, nil
  end

  if res.status == 200 then
    local body = json.decode(res.body)
    -- Return the full TokenValidationResponse
    return true, body
  else
    kong.log.warn("Auth validation failed with status: ", res.status)
    return false, nil
  end
end

function NasikoAuthHandler:access(config)
  -- Skip auth for health checks and auth service endpoints
  local path = kong.request.get_path()
  if path == "/health" or path == "/status" then
    return
  end

  -- Never require auth on CORS preflight
  local method = kong.request.get_method()
  if method == "OPTIONS" then
    return
  end

  -- Skip auth for auth service endpoints
  if path == "/auth/users/login" or
     path == "/auth/users/register" or
     path == "/api/v1/auth/github/login-user" or
     path == "/api/v1/auth/github/callback" or
     path == "/auth/users/check" then
    return
  end

  -- Get authorization header
  local auth_header = kong.request.get_header("authorization")
  if not auth_header then
    return kong.response.exit(401, {
      message = "Missing Authorization header"
    })
  end

  -- Extract Bearer token
  local token = auth_header:match("Bearer%s+(.+)")
  if not token then
    return kong.response.exit(401, {
      message = "Invalid Authorization header format. Expected 'Bearer <token>'"
    })
  end

  -- Validate token with auth service
  local auth_service_url = config.auth_service_url or "http://nasiko-auth:8001"
  local is_valid, token_validation_response = validate_token(
    auth_service_url,
    token,
    config.timeout or 5000
  )

  if not is_valid then
    return kong.response.exit(401, {
      message = "Invalid or expired token"
    })
  end

  -- Add TokenValidationResponse data to request headers for downstream services
  if token_validation_response then
    kong.service.request.set_header("X-Subject-ID", token_validation_response.subject_id or "")
    kong.service.request.set_header("X-Subject-Type", token_validation_response.subject_type or "")
    kong.service.request.set_header("X-Is-Super-User", tostring(token_validation_response.is_super_user or false))
    kong.service.request.set_header("X-Permissions", json.encode(token_validation_response.permissions or {}))
    kong.service.request.set_header("X-Valid", tostring(token_validation_response.valid or false))

    -- Forward the original Authorization header as Bearer token to downstream services
    kong.service.request.set_header("Authorization", "Bearer " .. token)
  end

  kong.log.info("Authentication successful for subject: ", token_validation_response.subject_id, " (", token_validation_response.subject_type, ")")
end

return NasikoAuthHandler
