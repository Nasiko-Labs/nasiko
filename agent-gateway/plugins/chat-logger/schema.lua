local typedefs = require "kong.db.schema.typedefs"

return {
  name = "chat-logger",
  fields = {
    { 
      config = {
        type = "record",
        fields = {
          {
            chat_service_url = {
              type = "string",
              default = "http://chat-history-service:8002",
              description = "URL of the chat history service"
            }
          },
          {
            timeout = {
              type = "number",
              default = 5000,
              description = "Timeout for chat service requests (milliseconds)"
            }
          }
        }
      }
    }
  }
}