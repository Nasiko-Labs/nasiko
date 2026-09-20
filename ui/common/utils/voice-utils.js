/**
 * Shared voice recording and transcription utilities
 */

import { apiFetch } from '/common/services/api.js';

/// Posts recorded audio to `POST /api/transcribe` and returns the text.
///
/// Every failure mode here is a deployment/environment condition rather than
/// something the user did, so each one gets a message that names the actual
/// cause — the raw body ("transcription API error") tells nobody anything.
export async function transcribeBlob(blob) {
  const form = new FormData();
  form.append('file', blob, 'audio.webm');
  const res = await apiFetch('/transcribe', { method: 'POST', body: form });

  if (!res.ok) {
    if (res.status === 503) {
      throw new Error('Speech-to-text is not configured on this deployment (no OpenAI API key).');
    }
    if (res.status === 502) {
      // The server reached its configured OpenAI-compatible endpoint and got a
      // non-2xx. Most often OPENAI_BASE_URL points at a provider that has no
      // /v1/audio/transcriptions route at all.
      throw new Error('The transcription provider rejected the request. Check that OPENAI_BASE_URL points at a service that supports audio transcription.');
    }
    throw new Error((await res.text()) || `Transcription failed (HTTP ${res.status})`);
  }

  const data = await res.json();
  return data.text;
}

export class VoiceRecorder {
  constructor() {
    this.isRecording = false;
    this.mediaRecorder = null;
    this.audioChunks = [];
    this.transcriptionCallback = null;
    this.stream = null;
  }

  setTranscriptionCallback(callback) {
    this.transcriptionCallback = callback;
  }

  async startRecording() {
    if (this.isRecording) return;

    // `navigator.mediaDevices` is undefined on an insecure origin, so the old
    // "use a modern browser" message pointed at the wrong thing entirely: the
    // usual cause is the control plane being served over plain HTTP on an IP.
    if (!window.isSecureContext) {
      throw new Error(
        "Microphone access needs a secure origin. Open this page over HTTPS (or via localhost) to record audio.",
      );
    }

    if (!navigator.mediaDevices || !navigator.mediaDevices.getUserMedia) {
      throw new Error(
        "Your browser does not support audio recording. Please use a modern browser like Chrome, Firefox, or Edge.",
      );
    }

    try {
      this.stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      this.mediaRecorder = new MediaRecorder(this.stream);

      this.mediaRecorder.ondataavailable = (event) => {
        this.audioChunks.push(event.data);
      };

      this.mediaRecorder.start();
      this.isRecording = true;
      this.audioChunks = [];
    } catch (error) {
      // DOMException names are the only reliable signal here — the messages
      // differ per browser.
      if (error?.name === "NotAllowedError" || error?.name === "SecurityError") {
        throw new Error(
          "Microphone permission was denied. Allow microphone access for this site in your browser settings, then try again.",
        );
      }
      if (error?.name === "NotFoundError" || error?.name === "DevicesNotFoundError") {
        throw new Error("No microphone was found on this device.");
      }
      throw new Error("Could not start recording: " + (error?.message || error));
    }
  }

  async stopRecording() {
    if (!this.isRecording || !this.mediaRecorder) return;

    return new Promise((resolve, reject) => {
      this.mediaRecorder.onstop = async () => {
        try {
          const audioBlob = new Blob(this.audioChunks, { type: "audio/webm" });

          if (this.transcriptionCallback) {
            const transcribedText = await this.transcriptionCallback(audioBlob);
            this.cleanup();
            resolve(transcribedText);
          } else {
            this.cleanup();
            reject(new Error("transcription callback not set"));
          }
        } catch (error) {
          this.cleanup();
          // Pass the message through unwrapped — `transcribeBlob` already
          // produces user-facing text, and prefixing it just buries the cause.
          reject(new Error(error?.message || "Transcription failed."));
        }
      };

      this.mediaRecorder.stop();
      this.isRecording = false;
    });
  }

  cleanup() {
    this.audioChunks = [];
    if (this.stream) {
      this.stream.getTracks().forEach((track) => track.stop());
      this.stream = null;
    }
    this.mediaRecorder = null;
  }
}
