// Its own module: name.ts reaches the shell through the deploy route's search schema, and copy.ts would come with it.
/** The zip check's one-line reasons (zipcheck.ts) and the name rule (name.ts), in the user's words. */
export const checkCopy = {
  overSize: (mb: number) => `Over ${mb} MB.`,
  overFiles: (n: number) => `Over ${n} files.`,
  overUnzipped: (mb: number) => `Over ${mb} MB unzipped.`,
  noDockerfile: 'No Dockerfile at the root.',
  noFrom: 'The Dockerfile has no FROM line.',
  noEntrypoint: 'No main.py, src/main.py, __main__.py or src/__main__.py.',
  noVersion: 'No version in the zip: set one below.',
  badVersion: (v: string) => `Version ${v} isn't x.y.z: set one below.`,
  nameLength: (max: number) => `Use 1–${max} characters.`,
  nameStart: 'Start with a letter, a digit or an underscore.',
  nameChars: 'Use only letters, digits, dots, dashes and underscores.',
  placeholderDir: 'my-agent',
}
