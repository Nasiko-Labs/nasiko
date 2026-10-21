// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/checkCopy.ts. Local edits: none beyond imports and
// Prettier.
// Its own module: name.ts reaches the shell through the deploy route's search schema, and copy.ts would come with it.
// Language-agnostic zip validation (legacy ui/ on development, 3aac84f66: 05f0bb2a6, 982901006): a Dockerfile
// CMD/ENTRYPOINT or a Python entrypoint passes.
/** The zip check's one-line reasons (zipcheck.ts) and the name rule (name.ts), in the user's words. */
export const checkCopy = {
  overSize: (mb: number) => `Over ${mb} MB.`,
  overFiles: (n: number) => `Over ${n} files.`,
  overUnzipped: (mb: number) => `Over ${mb} MB unzipped.`,
  noDockerfile: "No Dockerfile at the root.",
  noFrom: "The Dockerfile has no FROM line.",
  noEntrypoint: "No CMD or ENTRYPOINT in the Dockerfile, and no main.py / __main__.py.",
  noVersion: "No version in the zip: set one below.",
  badVersion: (v: string) => `Version ${v} isn't x.y.z: set one below.`,
  nameLength: (max: number) => `Use 1–${max} characters.`,
  nameStart: "Start with a letter, a digit or an underscore.",
  nameChars: "Use only letters, digits, dots, dashes and underscores.",
  placeholderDir: "my-agent",
};
