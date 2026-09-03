# Third-party attributions

## opencode agent (`oss/agents/opencode/`)

This agent's Docker image installs two third-party packages at build time
(`npm install -g` in `opencode/Dockerfile`) rather than vendoring their source
into this repository. Both are used unmodified, as external CLI/library
dependencies.

### opencode-ai (MIT)

[`opencode-ai`](https://www.npmjs.com/package/opencode-ai) is the `opencode` CLI,
upstream at [anomalyco/opencode](https://github.com/anomalyco/opencode). Installed
as `opencode-ai@1.18.16`; provides `opencode serve` (the agent's coding backend on
`localhost:4096`).

```
MIT License

Copyright (c) 2025 Anomaly

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

### a2a-opencode (MIT)

[`a2a-opencode`](https://www.npmjs.com/package/a2a-opencode) wraps the `opencode`
CLI in the A2A protocol, upstream at
[shashikanth-gs/a2a-wrapper](https://github.com/shashikanth-gs/a2a-wrapper) — a
third-party project, not affiliated with anomalyco or Nasiko. Installed as
`a2a-opencode@1.6.1`.

```
MIT License

Copyright (c) 2025 Shashikanth GS

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

Note: `opencode/Dockerfile` patches `a2a-opencode`'s installed
`dist/opencode/executor.js` post-install (see the `RUN` steps referencing that
path) to fix tool-call event reporting and a hang on an `@opencode-ai/sdk` call —
narrow, targeted patches to the installed package, not a fork or redistribution
of its source.
