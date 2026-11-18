/** Nasiko's public pages the app links to: the sidebar's help rows, the CLI guide and the assessment page. */
export const DOCS_URL = 'https://docs.nasiko.com'
export const CLI_DOCS_URL = 'https://docs.nasiko.com/reference/cli/overview'
/** A docs page under the CLI reference (`setup`, `operate`, …). */
export const cliDocsUrl = (page: string) => `https://docs.nasiko.com/reference/cli/${page}`
/** The team's booking page: the assessment's last step. */
export const EXPERT_CALL_URL = 'https://calendar.app.google/9e8V6Rtbm3gNz1x6A'
/**
 * The sales team's Calendly event (the same one as nasiko.com/demo): Contact sales' last step. Calendly reports each booking to the sales intake's
 * webhook (scripts/sales-intake/api/calendly-webhook.js), which sends the booked lead to Clay.
 */
export const SALES_CALL_URL = 'https://calendly.com/d/d4nr-czp-qw9/nasiko-intro'
/** The sidebar's Report an issue row: a mail to support. */
export const SUPPORT_MAIL_URL = 'mailto:support@nasiko.com'
