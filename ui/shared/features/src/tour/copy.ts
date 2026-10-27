/** The tour card's own words; each tour's steps are in `tours.ts`. */
export const copy = {
  step: (n: number, of: number) => `Step ${n} of ${of}`,
  next: 'Next',
  back: 'Back',
  done: 'Done',
  skip: 'Skip tour',
  // The app tour's first card, when it opened on its own: the one place to turn tours off.
  start: 'Show me around',
  noThanks: 'No thanks',
  noThanksHint: 'You can tour any page later from the ? button at the top right, or by pressing ?.',
}
