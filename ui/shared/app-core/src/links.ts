/** Nasiko's public pages the app links to: the sidebar's help rows, the CLI guide and the assessment page. */
export const DOCS_URL = 'https://docs.nasiko.com'
export const CLI_DOCS_URL = 'https://docs.nasiko.com/reference/cli/overview'
/** A docs page under the CLI reference (`setup`, `operate`, …). */
export const cliDocsUrl = (page: string) => `https://docs.nasiko.com/reference/cli/${page}`
/**
 * The sales team's Calendly event (the same one as nasiko.com/demo), embedded as the last step of Contact sales and of
 * the free TokenOps assessment. Calendly reports each booking to the sales intake's
 * webhook (scripts/sales-intake/api/calendly-webhook.js), which sends the booked lead to Clay.
 */
export const SALES_CALL_URL = 'https://calendly.com/d/d4nr-czp-qw9/nasiko-intro'
/** The sidebar's Report an issue row: a mail to support. */
export const SUPPORT_MAIL_URL = 'mailto:support@nasiko.com'
