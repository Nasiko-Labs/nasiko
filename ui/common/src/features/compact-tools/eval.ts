/** Output of compact_tools_eval for /tmp/compact-tools-eval.json (o200k_base). */
export const rows = [
  {
    id: 'ct-001',
    compacted: true,
    rendered_calls:
      '<<call create_calendar_event {"attendees":["riya@example.com"],' +
      '"start":"2026-10-05T15:00:00+05:30","title":"Design review"}>>',
    tool_names: ['create_calendar_event'],
    baseline_tokens: 204,
    compact_tokens: 157,
    tokens_saved: 47,
    reduction_percent: 23.03921568627451,
    roundtrip_success: true,
  },
  {
    id: 'ct-003',
    compacted: true,
    rendered_calls: '',
    tool_names: ['create_calendar_event'],
    baseline_tokens: 194,
    compact_tokens: 147,
    tokens_saved: 47,
    reduction_percent: 24.22680412371134,
    roundtrip_success: true,
  },
  {
    id: 'dc-002',
    decoded: {
      calls: [
        {
          function: {
            arguments: '{"start":"2026-10-04T10:00:00+05:30","title":"Retro"}',
            name: 'create_calendar_event',
          },
          id: 'call_0',
          type: 'function',
        },
      ],
    },
    tool_names: ['create_calendar_event'],
    decoder_ok: true,
    expected_error: false,
  },
] as const
