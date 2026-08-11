/**
 * Textarea with voice recording, optional file attachment, and submit on Enter.
 *
 * @element voice-input
 * @attr {string} placeholder - Textarea placeholder text
 * @attr {boolean} no-attachments - Hide the file attachment button
 * @attr {string} transcription-callback - Name of `window[callback](blob)` async function returning transcribed text
 * @prop {string} value - Get/set the textarea value
 * @fires voice-input-submit - User submits; `detail: { value: string, files: [] }` — bubbles
 */
import { VoiceRecorder } from '../utils/voice-utils.js';
import { icons } from '../utils/icons.js';
import { showToast } from '../utils/toast.js';
import './app-stack.js';
import './app-row.js';

import styles from './voice-input.css' with { type: 'css' };
document.adoptedStyleSheets = [...document.adoptedStyleSheets, styles];

export class VoiceInputForm extends HTMLElement {
  constructor() {
    super();
    this.voiceRecorder = new VoiceRecorder();
    this.attachedFiles = [];
    this.timerInterval = null;
    this.startTime = null;
    this.maxFileSize = 10 * 1024 * 1024;
  }

  #handleDocumentKeyDown = (e) => {
    const isR = e.key.toLowerCase() === "r" || e.code === "KeyR";
    if (e.key === "F8" || (e.altKey && isR)) {
      e.preventDefault();
      this.toggleRecording();
    }
    if (e.key === "/" && !e.ctrlKey && !e.metaKey && !e.altKey) {
      const tag = document.activeElement?.tagName;
      if (tag !== "INPUT" && tag !== "TEXTAREA" && !document.activeElement?.isContentEditable) {
        e.preventDefault();
        this.textarea.focus();
      }
    }
  };

  connectedCallback() {
    this.render();
    this.cacheElements();
    this.setupListeners();
    this.setState("idle");

    const callbackName = this.getAttribute("transcription-callback");
    if (callbackName && window[callbackName]) {
      this.voiceRecorder.setTranscriptionCallback(window[callbackName]);
    }
  }

  disconnectedCallback() {
    document.removeEventListener("keydown", this.#handleDocumentKeyDown);
    clearInterval(this.timerInterval);
  }

  render() {
    const noAttach = this.hasAttribute("no-attachments");
    const placeholder = this.getAttribute("placeholder") || "Type your message...";

    this.innerHTML = `
      <form class="voice-input" onsubmit="return false;">
        <div class="input-area" id="inputWrapper">
          <textarea
            class="textarea"
            id="textarea"
            rows="1"
            placeholder="${placeholder}"
          ></textarea>

          <div class="timer" id="timer">
            <span style="color:var(--color-error); animation: pulse-scale 1s infinite;">●</span>
            <span id="timerText">0.0s</span>
          </div>

          ${noAttach ? "" : `
          <button type="button" class="attach-btn" id="dropZone" title="Attach files" aria-label="Attach files">
            ${icons.paperclip("btn-icon", 16)}
            <input type="file" id="fileInput" multiple style="display:none">
          </button>`}

          <button type="button" class="record-btn" id="recordBtn">
            ${this.getMicIcon()}
          </button>

          <button type="submit" class="submit-icon" id="submitBtn" title="Send (Ctrl+Enter)">
            ${icons.arrowUp("btn-icon", 14)}
          </button>
        </div>

        ${noAttach ? "" : `<div class="footer"><div class="file-list" id="fileList"></div></div>`}
      </form>
    `;
  }

  cacheElements() {
    this.wrapper = this.querySelector("#inputWrapper");
    this.textarea = this.querySelector("#textarea");
    this.recordBtn = this.querySelector("#recordBtn");
    this.timerEl = this.querySelector("#timer");
    this.timerText = this.querySelector("#timerText");
    this.fileList = this.querySelector("#fileList");
    this.dropZone = this.querySelector("#dropZone");
    this.fileInput = this.querySelector("#fileInput");
    this.submitBtn = this.querySelector("#submitBtn");
  }

  setupListeners() {
    this.recordBtn.addEventListener("click", () => this.toggleRecording());
    this.submitBtn.addEventListener("click", () => this.handleSubmit());

    this.textarea.addEventListener("keydown", (e) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        e.stopPropagation();
        this.handleSubmit();
      }
    });

    document.removeEventListener("keydown", this.#handleDocumentKeyDown);
    document.addEventListener("keydown", this.#handleDocumentKeyDown);

    this.dropZone?.addEventListener("click", () => this.fileInput.click());
    this.fileInput?.addEventListener("change", (e) => this.handleFiles(e.target.files));

    ["dragenter", "dragover"].forEach((name) => {
      this.wrapper?.addEventListener(name, (e) => {
        e.preventDefault();
        this.wrapper.classList.add("drag-over");
      });
    });
    ["dragleave", "drop"].forEach((name) => {
      this.wrapper?.addEventListener(name, (e) => {
        e.preventDefault();
        this.wrapper.classList.remove("drag-over");
      });
    });
    this.wrapper?.addEventListener("drop", (e) => this.handleFiles(e.dataTransfer.files));
  }

  setState(state) {
    this.wrapper.dataset.state = state;
    this.recordBtn.dataset.state = state;

    switch (state) {
      case "idle":
        this.recordBtn.disabled = false;
        this.recordBtn.innerHTML = this.getMicIcon();
        this.recordBtn.title = "Start Recording (F8 or Alt+R)";
        this.timerEl.classList.remove("visible");
        this.submitBtn.disabled = false;
        break;
      case "recording":
        this.recordBtn.innerHTML = this.getStopIcon();
        this.recordBtn.title = "Stop Recording (F8)";
        this.timerEl.classList.add("visible");
        this.startTimer();
        break;
      case "transcribing":
        this.recordBtn.disabled = true;
        this.recordBtn.innerHTML = `<div class="spinner"></div>`;
        this.timerEl.classList.add("visible");
        this.stopTimer(false);
        break;
      case "loading":
        this.submitBtn.disabled = true;
        this.recordBtn.disabled = true;
        this.timerEl.classList.remove("visible");
        break;
    }
  }

  async toggleRecording() {
    if (this.voiceRecorder.isRecording) {
      this.setState("transcribing");
      try {
        const text = await this.voiceRecorder.stopRecording();
        if (text) this.insertText(text);
      } catch (err) {
        showToast(err.message);
      } finally {
        this.setState("idle");
      }
    } else {
      try {
        await this.voiceRecorder.startRecording();
        this.setState("recording");
      } catch (err) {
        // startRecording's messages already name the cause (insecure origin,
        // denied permission, no device) — don't bury them behind a prefix.
        showToast(err.message);
      }
    }
  }

  handleSubmit() {
    const query = this.textarea.value.trim();
    if (!query && this.attachedFiles.length === 0) return;
    const files = [...this.attachedFiles];
    this.setState("loading");
    this.dispatchEvent(
      new CustomEvent("voice-input-submit", {
        bubbles: true,
        detail: { value: query, files },
      }),
    );
  }

  setLoading(isLoading) {
    this.setState(isLoading ? "loading" : "idle");
  }

  reset() {
    this.textarea.value = "";
    this.attachedFiles = [];
    this.renderFiles();
  }

  insertText(text) {
    const start = this.textarea.selectionStart;
    const end = this.textarea.selectionEnd;
    const current = this.textarea.value;
    const before = current.substring(0, start);
    const after = current.substring(end);
    const spacing = before.length > 0 && !before.endsWith(" ") && !before.endsWith("\n") ? " " : "";
    this.textarea.value = before + spacing + text + after;
    const newPos = start + spacing.length + text.length;
    this.textarea.focus();
    this.textarea.setSelectionRange(newPos, newPos);
  }

  startTimer() {
    this.startTime = Date.now();
    this.timerText.innerText = "0.0s";
    clearInterval(this.timerInterval);
    this.timerInterval = setInterval(() => {
      const diff = (Date.now() - this.startTime) / 1000;
      this.timerText.innerText = diff.toFixed(1) + "s";
    }, 100);
  }

  stopTimer(reset = true) {
    clearInterval(this.timerInterval);
    if (reset) this.timerText.innerText = "0.0s";
  }

  handleFiles(fileList) {
    Array.from(fileList).forEach((file) => {
      if (file.size > this.maxFileSize) {
        showToast(`File too large: ${file.name}`);
        return;
      }
      this.attachedFiles.push({
        id: Math.random().toString(36).slice(2),
        file,
        name: file.name,
        size: file.size,
        type: file.type,
      });
    });
    this.renderFiles();
  }

  renderFiles() {
    if (!this.fileList) return;
    this.fileList.innerHTML = this.attachedFiles.map((f) => `
      <div class="file-item">
        <span>${f.name} (${(f.size / 1024).toFixed(1)}KB)</span>
        <button class="file-remove" data-id="${f.id}" title="Remove file" aria-label="Remove file">
          ${icons.x("", 14)}
        </button>
      </div>
    `).join("");

    this.querySelectorAll(".file-remove").forEach((btn) => {
      btn.onclick = () => this.removeFile(btn.dataset.id);
    });
  }

  removeFile(id) {
    this.attachedFiles = this.attachedFiles.filter((f) => f.id !== id);
    this.renderFiles();
  }

  getMicIcon() {
    return icons.mic("btn-icon", 18);
  }

  getStopIcon() {
    return icons.square("btn-icon", 12);
  }

  get value() {
    return this.textarea.value;
  }
  set value(val) {
    this.textarea.value = val;
  }
}

customElements.define("voice-input", VoiceInputForm);
