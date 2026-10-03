Feature: Public sample round trip

  The fixture tests/fixtures/compact-tools-eval.json is the published
  compact-tools-eval-v1 sample. These scenarios do not special-case an id.
  They render that case's expected calls and compare the decoded JSON.

  Scenario: The sample lists the calendar and email tools
    Given the public sample
    Then the sample includes a tool named "create_calendar_event"
    And the sample includes a tool named "send_email"

  Scenario Outline: Expected calls round-trip through the compact grammar
    Given the public sample
    When case "<id>" is round-tripped
    Then the round trip matches case "<id>"

    Examples:
      | id     | what it checks                                      |
      | ct-001 | one design-review call                              |
      | ct-002 | email then calendar, including duration and privacy |
      | ct-003 | a question with no call                             |

  Scenario Outline: Published decoder chunks match the sample
    Given the public sample
    When decoder case "<id>" is fed as published chunks
    Then the decoded chunks match case "<id>"

    Examples:
      | id     | what it checks                          |
      | dc-001 | one complete call                       |
      | dc-002 | a marker split across chunks            |
      | dc-003 | a closer inside a string argument       |
      | dc-004 | unknown tool                            |
      | dc-005 | missing required field and a bad enum   |
