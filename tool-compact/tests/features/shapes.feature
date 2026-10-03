Feature: Nested objects, numbers, and booleans survive the compact form

  Background:
    Given a tool named "schedule" described as "Schedule a visit" with this schema:
      """
      {
        "type": "object",
        "additionalProperties": false,
        "properties": {
          "place": {
            "type": "object",
            "properties": {
              "city": {"type": "string"},
              "rooms": {"type": "integer"}
            },
            "required": ["city"]
          },
          "flags": {"type": "array", "items": {"type": "boolean"}},
          "score": {"type": "number"},
          "ok": {"type": "boolean"}
        },
        "required": ["place", "ok"]
      }
      """

  Scenario: The signature shows the nested object and the scalar types
    When the tools are encoded
    Then the signature contains "place:{city:str, rooms?:int}"
    And the signature contains "flags?:[bool]"
    And the signature contains "score?:num"
    And the signature contains "ok:bool"
    When the compact text is decoded back into schemas
    Then decoded tool 1 property "ok" has type "boolean"
    And decoded tool 1 property "score" has type "number"
    And decoded tool 1 property "flags" items are "boolean"
    And decoded tool 1 nested property "place" "city" has type "string"
    And decoded tool 1 nested property "place" "rooms" has type "integer"

  Scenario: A complete visit call is accepted
    When the model replies:
      """
      <<call schedule {"place":{"city":"Pune","rooms":2},"flags":[true,false],"score":1.5,"ok":true}>>
      """
    Then there are 1 calls
    And call 1 is named "schedule"
    And call 1 argument "ok" json is "true"
    And call 1 argument "score" json is "1.5"
    And call 1 argument "place" json is:
      """
      {"city":"Pune","rooms":2}
      """

  Scenario: Optional score and flags may be omitted
    When the model replies:
      """
      <<call schedule {"place":{"city":"Pune"},"ok":false}>>
      """
    Then there are 1 calls
    And call 1 argument "ok" json is "false"

  Scenario: A nested object missing its required city is rejected
    When the model replies:
      """
      <<call schedule {"place":{"rooms":2},"ok":true}>>
      """
    Then decoding fails with missing field "city"

  Scenario: A nested place sent as a string is rejected
    When the model replies:
      """
      <<call schedule {"place":"Pune","ok":true}>>
      """
    Then decoding fails with wrong type for "place"

  Scenario: A boolean array item sent as a string is rejected
    When the model replies:
      """
      <<call schedule {"place":{"city":"Pune"},"flags":["yes"],"ok":true}>>
      """
    Then decoding fails with wrong type for "flags"

  Scenario: A number sent as a string is rejected
    When the model replies:
      """
      <<call schedule {"place":{"city":"Pune"},"score":"high","ok":true}>>
      """
    Then decoding fails with wrong type for "score"

  Scenario: A boolean sent as a string is rejected
    When the model replies:
      """
      <<call schedule {"place":{"city":"Pune"},"ok":"yes"}>>
      """
    Then decoding fails with wrong type for "ok"

  Scenario: A field the schema does not list is rejected
    When the model replies:
      """
      <<call schedule {"place":{"city":"Pune"},"ok":true,"note":"hi"}>>
      """
    Then decoding fails with wrong type for "note"

  Scenario: An integer nested field sent as a string is rejected
    When the model replies:
      """
      <<call schedule {"place":{"city":"Pune","rooms":"2"},"ok":true}>>
      """
    Then decoding fails with wrong type for "rooms"
