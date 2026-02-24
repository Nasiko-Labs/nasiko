local ChatLoggerHandler = {}
local cjson = require "cjson"
local http = require "resty.http"

ChatLoggerHandler.PRIORITY = 1000
ChatLoggerHandler.VERSION = "1.0.0"

function ChatLoggerHandler:access(plugin_conf)
  -- Store request body for later use in response handler
  kong.ctx.plugin.request_body = kong.request.get_raw_body()
  kong.ctx.plugin.start_time = ngx.now()
end

function ChatLoggerHandler:body_filter(plugin_conf)
  -- Capture response body
  local chunk = ngx.arg[1]
  local eof = ngx.arg[2]
  
  -- Initialize response body buffer if not exists
  if not kong.ctx.plugin.response_body then
    kong.ctx.plugin.response_body = ""
  end
  
  -- Append chunk to response body
  if chunk then
    kong.ctx.plugin.response_body = kong.ctx.plugin.response_body .. chunk
  end
  
  -- If this is the last chunk, store the complete response
  if eof then
    kong.ctx.plugin.complete_response = kong.ctx.plugin.response_body
  end
end

function ChatLoggerHandler:log(plugin_conf)
  local request_body = kong.ctx.plugin.request_body
  local response_body = kong.ctx.plugin.complete_response
  
  -- Skip if no request or response body
  if not request_body or not response_body then
    kong.log.debug("Chat Logger: Missing request or response body, skipping")
    return
  end
  
  -- Parse JSON bodies
  local request_data, response_data
  
  local success, err = pcall(function()
    request_data = cjson.decode(request_body)
    response_data = cjson.decode(response_body)
  end)
  
  if not success then
    kong.log.err("Chat Logger: Failed to parse JSON - ", err)
    return
  end
  
  -- Check if this is a JSONRPC message/send request
  if not request_data.method or request_data.method ~= "message/send" then
    kong.log.debug("Chat Logger: Not a message/send request, skipping")
    return
  end
  
  -- Check if we have the required session ID
  if not request_data.id then
    kong.log.debug("Chat Logger: No session ID in request, skipping")
    return
  end
  
  -- Create log payload
  local log_payload = {
    request_data = request_data,
    response_data = response_data,
    timestamp = os.date("!%Y-%m-%dT%H:%M:%SZ"),
    processing_time_ms = math.floor((ngx.now() - kong.ctx.plugin.start_time) * 1000)
  }
  
  -- Send to chat history service asynchronously
  local ok, err = ngx.timer.at(0, send_to_chat_service, plugin_conf, log_payload)
  if not ok then
    kong.log.err("Chat Logger: Failed to create timer - ", err)
  end
end

function send_to_chat_service(premature, plugin_conf, log_payload)
  if premature then
    return
  end
  
  local httpc = http.new()
  httpc:set_timeout(plugin_conf.timeout or 5000)
  
  local chat_service_url = plugin_conf.chat_service_url or "http://chat-history-service:8002"
  local endpoint = chat_service_url .. "/log-chat"
  
  local res, err = httpc:request_uri(endpoint, {
    method = "POST",
    headers = {
      ["Content-Type"] = "application/json",
    },
    body = cjson.encode(log_payload),
  })
  
  if not res then
    kong.log.err("Chat Logger: Failed to send to chat service - ", err)
  elseif res.status >= 400 then
    kong.log.err("Chat Logger: Chat service returned error - ", res.status, " ", res.body)
  else
    kong.log.info("Chat Logger: Successfully logged chat for session ", log_payload.request_data.id)
  end
  
  httpc:close()
end

return ChatLoggerHandler