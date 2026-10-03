Feature: Stream decoder holds a call until the closer arrives

  Background:
    Given the sample tool "create_calendar_event"

  Scenario: A marker split across chunks emits one call at the end
    When the stream receives chunk "<<ca"
    And the stream receives this chunk:
      """
      ll create_calendar_event {"title":"Ret
      """
    And the stream receives this chunk:
      """
      ro","start":"2026-10-04T10:00:00+05:30"}>
      """
    And the stream receives chunk ">"
    And the stream is finished
    Then chunk 1 emits 0 calls
    And chunk 2 emits 0 calls
    And chunk 3 emits 0 calls
    And chunk 4 emits 1 call
    And call 1 is named "create_calendar_event"
    And call 1 argument "title" is "Retro"

  Scenario: One chunk matches a direct decode
    When the model replies:
      """
      <<call create_calendar_event {"title":"Design review","start":"2026-10-05T15:00:00+05:30","attendees":["riya@example.com"]}>>
      """
    And that reply is pushed as one stream chunk
    Then the stream calls match the decoded calls

  Scenario: A bad enum split across chunks is an error and not a call
    When the stream receives this chunk:
      """
      <<call create_calendar_event {"title":"Retro","start":"2026-10-04T10:00:00+05:30","visibility":"sec
      """
    And the stream receives this chunk:
      """
      ret"}>>
      """
    And the stream is finished
    Then chunk 1 emits 0 calls
    And chunk 2 emits 0 calls
    And the stream fails with a bad enum for "visibility"

  Scenario: An unfinished marker fails when the stream ends
    When the stream receives this chunk:
      """
      <<call create_calendar_event {"title":"Retro"}
      """
    And the stream is finished
    Then the stream fails as malformed

  Scenario: A plain answer streamed in one chunk has no calls
    When the stream receives chunk "What's the weather?"
    And the stream is finished
    Then there are 0 calls
