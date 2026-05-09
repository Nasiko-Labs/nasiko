return {
  name = "nasiko-auth",
  fields = {
    {
      config = {
        type = "record",
        fields = {
          {
            auth_service_url = {
              type = "string",
              required = true,
              description = "URL of the authentication service"
            }
          },
          {
            timeout = {
              type = "number",
              default = 5000,
              description = "Timeout for auth service calls in milliseconds"
            }
          }
        }
      }
    }
  }
}
