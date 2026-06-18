const styles = new CSSStyleSheet();
styles.replaceSync(`@scope (publish-page) {
  :scope {
    display: block;
    max-width: 560px;
    margin: 0 auto;
    padding: var(--space-xl) var(--space-md);
  }
  .title { font-size: var(--font-size-xl); font-weight: 500; margin-bottom: var(--space-lg); }
  form { display: flex; flex-direction: column; gap: var(--space-sm); }
  label { font-size: var(--font-size-xs); font-weight: 500; color: var(--color-text-muted); text-transform: uppercase; letter-spacing: 0.03em; }
  input, select {
    width: 100%;
    padding: var(--space-xs) var(--space-sm);
    border: 1px solid var(--color-border);
    border-radius: var(--radius-md);
    background: var(--color-bg-surface);
    color: var(--color-text-main);
    font-size: var(--font-size-sm);
    font-family: inherit;
    appearance: none;
    -webkit-appearance: none;
  }
  select {
    background-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='12' height='12' viewBox='0 0 24 24' fill='none' stroke='%2364748b' stroke-width='2'%3E%3Cpath d='m6 9 6 6 6-6'/%3E%3C/svg%3E");
    background-repeat: no-repeat;
    background-position: right 10px center;
    padding-right: 28px;
  }
  input:focus, select:focus {
    outline: none;
    border-color: var(--color-primary);
    box-shadow: 0 0 0 3px var(--color-primary-ring);
  }
  .field { display: flex; flex-direction: column; gap: 3px; }
  .row { display: grid; grid-template-columns: 1fr 1fr; gap: var(--space-sm); }
  .row-3 { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: var(--space-sm); }
  .hint { font-size: var(--font-size-xs); color: var(--color-text-muted); }
  .file-drop {
    border: 1.5px dashed var(--color-border);
    border-radius: var(--radius-md);
    padding: var(--space-md);
    text-align: center;
    color: var(--color-text-muted);
    font-size: var(--font-size-xs);
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }
  .file-drop:hover, .file-drop.dragover {
    border-color: var(--color-primary);
    background: var(--color-primary-subtle);
  }
  .file-drop .selected { color: var(--color-text-main); font-weight: 500; }
  .submit-btn {
    padding: var(--space-xs) var(--space-lg);
    background: var(--color-primary);
    color: var(--color-on-primary);
    border: none;
    border-radius: var(--radius-md);
    font-size: var(--font-size-sm);
    font-weight: 500;
    cursor: pointer;
    margin-top: var(--space-sm);
  }
  .submit-btn:hover { background: var(--color-primary-hover); }
  .submit-btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .msg-success { color: var(--color-success); font-size: var(--font-size-xs); margin-top: var(--space-xs); }
  .msg-error { color: var(--color-error); font-size: var(--font-size-xs); margin-top: var(--space-xs); }
}`);
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

class PublishPage extends HTMLElement {
  #file = null;

  connectedCallback() {
    this.innerHTML = `
      <h1 class="title">Publish Artifact</h1>
      <form id="publish-form">
        <div class="row-3">
          <div class="field">
            <label>Owner</label>
            <auto-complete id="pub-owner-ac" placeholder="nasiko" filter-function="filterOwners"></auto-complete>
          </div>
          <div class="field">
            <label>Name</label>
            <input name="name" placeholder="my-agent" required />
          </div>
          <div class="field">
            <label>Version</label>
            <input name="version" value="1.0.0" required />
          </div>
        </div>
        <div class="row">
          <div class="field">
            <label>Type</label>
            <select name="artifact_type">
              <option value="agent">Agent</option>
              <option value="skill">Skill</option>
              <option value="tool">Tool</option>
            </select>
          </div>
          <div class="field">
            <label>Framework</label>
            <auto-complete id="pub-framework-ac" placeholder="openai" filter-function="filterFrameworks"></auto-complete>
          </div>
        </div>
        <div class="field">
          <label>Description</label>
          <input name="description" placeholder="What does this artifact do?" />
        </div>
        <div class="row">
          <div class="field">
            <label>Tags</label>
            <input name="tags" placeholder="research, streaming" />
          </div>
          <div class="field">
            <label>License</label>
            <input name="license" value="MIT" />
          </div>
        </div>
        <div class="field">
          <label>Artifact archive (.tar.gz)</label>
          <div class="file-drop" id="file-drop">Drop or click to select</div>
          <input type="file" id="pub-file" accept=".tar.gz,.tgz" hidden />
        </div>
        <button type="submit" class="submit-btn">Publish</button>
        <div id="publish-msg"></div>
      </form>
    `;

    this.#setupFileDrop();
    this.querySelector('#publish-form').addEventListener('submit', (e) => this.#handleSubmit(e));
  }

  #setupFileDrop() {
    const drop = this.querySelector('#file-drop');
    const input = this.querySelector('#pub-file');
    drop.addEventListener('click', () => input.click());
    drop.addEventListener('dragover', (e) => { e.preventDefault(); drop.classList.add('dragover'); });
    drop.addEventListener('dragleave', () => drop.classList.remove('dragover'));
    drop.addEventListener('drop', (e) => {
      e.preventDefault();
      drop.classList.remove('dragover');
      if (e.dataTransfer.files.length) this.#setFile(e.dataTransfer.files[0]);
    });
    input.addEventListener('change', () => { if (input.files.length) this.#setFile(input.files[0]); });
  }

  #setFile(file) {
    this.#file = file;
    this.querySelector('#file-drop').innerHTML = `<span class="selected">${file.name}</span> (${(file.size / 1024).toFixed(1)} KB)`;
  }

  async #handleSubmit(e) {
    e.preventDefault();
    const form = e.target;
    const btn = form.querySelector('.submit-btn');
    const msg = this.querySelector('#publish-msg');
    btn.disabled = true;
    msg.textContent = '';
    msg.className = '';

    const owner = this.querySelector('#pub-owner-ac')?.value || 'nasiko';
    const framework = this.querySelector('#pub-framework-ac')?.value || '';
    const tagsRaw = form.tags.value;
    const tags = tagsRaw ? tagsRaw.split(',').map(t => t.trim()).filter(Boolean) : [];

    if (!this.#file) {
      msg.className = 'msg-error';
      msg.textContent = 'Select an artifact archive';
      btn.disabled = false;
      return;
    }

    try {
      const result = await window.publishArtifact({
        owner, name: form.name.value, version: form.version.value,
        artifact_type: form.artifact_type.value, framework,
        description: form.description.value || null, tags,
        license: form.license.value || null,
      });
      msg.className = 'msg-success';
      msg.textContent = `Published ${result.artifact.owner}/${result.artifact.name}:${result.artifact.version}`;
      form.reset();
      this.#file = null;
      this.querySelector('#file-drop').innerHTML = 'Drop or click to select';
    } catch (err) {
      msg.className = 'msg-error';
      msg.textContent = err.message;
    } finally {
      btn.disabled = false;
    }
  }
}

customElements.define('publish-page', PublishPage);
