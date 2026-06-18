/**
 * Shared voice recording and transcription utilities
 */

import { icons } from '/common/utils/icons.js';

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
      throw new Error("failed to start recording: " + error.message);
    }
  }

  async stopRecording() {
    if (!this.isRecording || !this.mediaRecorder) return;

    return new Promise((resolve, reject) => {
      this.mediaRecorder.onstop = async () => {
        try {
          const audioBlob = new Blob(this.audioChunks, { type: "audio/wav" });
          const formData = new FormData();
          const recordingFileName = `recording-${new Date().toISOString().replace(/:/g, "-")}.wav`;
          formData.append("audio", audioBlob, recordingFileName);

          if (this.transcriptionCallback) {
            const transcribedText = await this.transcriptionCallback(formData);
            this.cleanup();
            resolve(transcribedText);
          } else {
            this.cleanup();
            reject(new Error("transcription callback not set"));
          }
        } catch (error) {
          this.cleanup();
          reject(new Error("transcription failed: " + error.message));
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
