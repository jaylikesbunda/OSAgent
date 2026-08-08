window.OSA = window.OSA || {};

OSA.speechPlaybackGeneration = OSA.speechPlaybackGeneration || 0;
OSA.bumpSpeechPlaybackGeneration = function() {
    OSA.speechPlaybackGeneration = (OSA.speechPlaybackGeneration || 0) + 1;
    return OSA.speechPlaybackGeneration;
};
OSA.getSpeechPlaybackGeneration = function() {
    return OSA.speechPlaybackGeneration || 0;
};

OSA.normalizeSttProvider = function(provider) {
    if (provider === 'whisper') return 'whisper-local';
    if (provider === 'whisper-api') return 'browser';
    return provider || 'browser';
};

OSA.normalizeTtsProvider = function(provider) {
    if (provider === 'piper') return 'piper-local';
    return provider || 'browser';
};

OSA.normalizeVoiceConfig = function(voiceConfig) {
    if (!voiceConfig) return null;
    return {
        ...voiceConfig,
        stt_provider: OSA.normalizeSttProvider(voiceConfig.stt_provider),
        tts_provider: OSA.normalizeTtsProvider(voiceConfig.tts_provider)
    };
};

// Microphone selection.
//
// Kept in localStorage rather than the server config: device ids are scoped to
// one browser profile and are meaningless on any other machine, so they do not
// belong in a config file that gets copied around.
OSA.getPreferredInputDevice = function() {
    try {
        return localStorage.getItem('osa.voice.inputDevice') || '';
    } catch (err) {
        return '';
    }
};

OSA.setPreferredInputDevice = function(deviceId) {
    try {
        if (deviceId) {
            localStorage.setItem('osa.voice.inputDevice', deviceId);
        } else {
            localStorage.removeItem('osa.voice.inputDevice');
        }
    } catch (err) {
        console.warn('Could not persist microphone choice:', err);
    }
};

OSA.listInputDevices = async function() {
    if (!navigator.mediaDevices?.enumerateDevices) return [];
    try {
        const devices = await navigator.mediaDevices.enumerateDevices();
        return devices.filter(device => device.kind === 'audioinput');
    } catch (err) {
        console.warn('Could not enumerate microphones:', err);
        return [];
    }
};

// Browsers hide device labels until the page has been granted mic access once,
// so an unprimed picker shows a list of blank entries.
OSA.inputDeviceLabelsAvailable = async function() {
    const devices = await OSA.listInputDevices();
    return devices.length > 0 && devices.some(device => !!device.label);
};

OSA.buildAudioConstraints = function() {
    const deviceId = OSA.getPreferredInputDevice();
    const audio = {
        // Without cancellation the mic hears our own TTS, which makes talking
        // over the agent feed its own speech back into the recogniser.
        echoCancellation: true,
        noiseSuppression: true,
        autoGainControl: true,
    };
    if (deviceId) {
        audio.deviceId = { ideal: deviceId };
    }
    return { audio };
};

OSA.isBrowserSpeechRecognitionSupported = function() {
    return 'webkitSpeechRecognition' in window || 'SpeechRecognition' in window;
};

OSA.stopLocalMediaStream = function() {
    const stream = OSA.getMediaStream();
    if (stream) {
        stream.getTracks().forEach(track => track.stop());
    }
    OSA.setMediaStream(null);
};

OSA.resetLocalRecorder = function() {
    OSA.stopPartialTranscription?.();
    OSA.setMediaRecorder(null);
    OSA.setMediaChunks([]);
    OSA.stopLevelMonitor?.();
    OSA.stopLocalMediaStream();
};

OSA.setVoiceStatus = function(message, tone = 'idle') {
    OSA.setVoiceStatusMessage(message || '');
    const status = document.getElementById('voice-status');
    if (!status) return;

    if (!message) {
        status.innerHTML = '';
        status.classList.add('hidden');
        status.dataset.state = 'hidden';
        return;
    }

    status.innerHTML = '';
    const text = document.createElement('span');
    text.className = 'voice-status-text';
    text.textContent = message;

    const dismiss = document.createElement('button');
    dismiss.type = 'button';
    dismiss.className = 'voice-status-dismiss';
    dismiss.setAttribute('aria-label', 'Dismiss voice status');
    dismiss.textContent = 'x';
    dismiss.addEventListener('click', () => OSA.clearVoiceStatus());

    status.appendChild(text);
    status.appendChild(dismiss);
    status.classList.remove('hidden');
    status.dataset.state = tone;
};

OSA.clearVoiceStatus = function() {
    OSA.setVoiceStatus('');
};

OSA.updateVoiceStatus = function(message, tone = 'idle') {
    if (message) {
        OSA.setVoiceStatus(message, tone);
        return;
    }

    const voiceConfig = OSA.normalizeVoiceConfig(OSA.getVoiceConfig());
    if (!voiceConfig?.enabled) {
        OSA.clearVoiceStatus();
        return;
    }

    if (OSA.getIsTranscribing()) {
        OSA.setVoiceStatus('Transcribing with Local Whisper...', 'busy');
        return;
    }

    if (OSA.getIsRecording()) {
        const sttProvider = OSA.normalizeSttProvider(voiceConfig.stt_provider);
        if (sttProvider === 'whisper-local') {
            OSA.setVoiceStatus('Listening for Local Whisper... click the mic again to stop.', 'recording');
        } else {
            OSA.setVoiceStatus('Listening in the browser... click the mic again to stop.', 'recording');
        }
        return;
    }

    OSA.clearVoiceStatus();
};

OSA.initVoice = function() {
    const config = OSA.getCachedConfig();
    if (!config?.voice) {
        OSA.fetchWithAuth('/api/config')
        .then(res => res.json())
        .then(cfg => {
            const voiceConfig = OSA.normalizeVoiceConfig(cfg.voice);
            OSA.setVoiceConfig(voiceConfig);
            if (cfg) {
                OSA.setCachedConfig({ ...cfg, voice: voiceConfig });
            }
            if (voiceConfig?.enabled && OSA.normalizeSttProvider(voiceConfig.stt_provider) === 'browser') {
                OSA.initSpeechRecognition();
            }
            OSA.setTtsEnabled(!!voiceConfig?.auto_speak);
            OSA.updateVoiceButtons();
            OSA.ensureVoiceModeSynced();
        })
        .catch(err => console.error('Failed to load voice config:', err));
        return;
    }

    const voiceConfig = OSA.normalizeVoiceConfig(config.voice);
    OSA.setVoiceConfig(voiceConfig);
    OSA.setCachedConfig({ ...config, voice: voiceConfig });
    if (voiceConfig?.enabled && OSA.normalizeSttProvider(voiceConfig.stt_provider) === 'browser') {
        OSA.initSpeechRecognition();
    }
    OSA.setTtsEnabled(!!voiceConfig?.auto_speak);
    OSA.updateVoiceButtons();
    OSA.refreshWhisperCapabilities();
    OSA.ensureVoiceModeSynced();
};

OSA.initSpeechRecognition = function() {
    if (!OSA.isBrowserSpeechRecognitionSupported()) {
        console.warn('Speech recognition not supported in this browser');
        return;
    }
    
    const SpeechRecognition = window.SpeechRecognition || window.webkitSpeechRecognition;
    const recognition = new SpeechRecognition();
    recognition.continuous = false;
    recognition.interimResults = true;
    recognition.lang = OSA.getVoiceConfig()?.language || 'en';
    
    recognition.onresult = (event) => {
        const results = Array.from(event.results);
        const transcript = results.map(result => result[0].transcript).join('');

        const input = document.getElementById('message-input');
        if (input) input.value = transcript;

        // Surface interim words while speaking. Browser recognition already
        // produces these; showing them is what makes the latency feel gone.
        OSA.setVoiceModeTranscript(transcript);

        const isFinal = results.length > 0 && results[results.length - 1].isFinal;
        if (!isFinal && transcript.trim()) {
            OSA.setVoiceStatus(`Hearing: "${transcript.trim()}"`, 'recording');
        }
    };
    
    recognition.onend = () => {
        if (OSA.getIsRecording()) {
            OSA.setIsRecording(false);
            OSA.updateMicButton();

            if (OSA.shouldAutoSendTranscript()) {
                const input = document.getElementById('message-input');
                if (input && input.value.trim()) {
                    OSA.sendMessage();
                }
            }
        }
    };
    
    recognition.onerror = (event) => {
        console.error('Speech recognition error:', event.error);
        OSA.setIsRecording(false);
        OSA.updateMicButton();
        OSA.setVoiceStatus(`Browser speech recognition error: ${event.error}`, 'error');
    };

    OSA.setRecognition(recognition);
};

// Whether a finished transcript should go straight to the agent.
//
// Voice mode always sends: it is a hands-free surface, and stopping to click
// Send would defeat the point. Outside it, the inline mic stays dictation-only
// unless the user opted into auto-send in settings.
OSA.shouldAutoSendTranscript = function() {
    if (OSA.isVoiceModeOpen?.()) return true;
    return !!OSA.getVoiceConfig()?.auto_send;
};

OSA.applyTranscriptToInput = function(text) {
    const input = document.getElementById('message-input');
    const transcript = (text || '').trim();
    if (!input || !transcript) return;

    const wasPartial = input.dataset.partialTranscript === '1';
    delete input.dataset.partialTranscript;

    const current = wasPartial ? '' : input.value.trim();
    input.value = current ? `${current} ${transcript}`.trim() : transcript;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    input.focus();
};

OSA.arrayBufferToBase64 = function(buffer) {
    const bytes = buffer instanceof Uint8Array ? buffer : new Uint8Array(buffer);
    let binary = '';
    const chunkSize = 0x8000;

    for (let i = 0; i < bytes.length; i += chunkSize) {
        const chunk = bytes.subarray(i, i + chunkSize);
        binary += String.fromCharCode.apply(null, chunk);
    }

    return btoa(binary);
};

OSA.encodeAudioBufferToWav = function(audioBuffer) {
    const channelCount = audioBuffer.numberOfChannels;
    const frameCount = audioBuffer.length;
    const sampleRate = audioBuffer.sampleRate;
    const mono = new Float32Array(frameCount);

    for (let channel = 0; channel < channelCount; channel += 1) {
        const channelData = audioBuffer.getChannelData(channel);
        for (let i = 0; i < frameCount; i += 1) {
            mono[i] += channelData[i] / channelCount;
        }
    }

    const buffer = new ArrayBuffer(44 + mono.length * 2);
    const view = new DataView(buffer);

    const writeString = function(offset, value) {
        for (let i = 0; i < value.length; i += 1) {
            view.setUint8(offset + i, value.charCodeAt(i));
        }
    };

    writeString(0, 'RIFF');
    view.setUint32(4, 36 + mono.length * 2, true);
    writeString(8, 'WAVE');
    writeString(12, 'fmt ');
    view.setUint32(16, 16, true);
    view.setUint16(20, 1, true);
    view.setUint16(22, 1, true);
    view.setUint32(24, sampleRate, true);
    view.setUint32(28, sampleRate * 2, true);
    view.setUint16(32, 2, true);
    view.setUint16(34, 16, true);
    writeString(36, 'data');
    view.setUint32(40, mono.length * 2, true);

    let offset = 44;
    for (let i = 0; i < mono.length; i += 1) {
        const sample = Math.max(-1, Math.min(1, mono[i]));
        view.setInt16(offset, sample < 0 ? sample * 0x8000 : sample * 0x7FFF, true);
        offset += 2;
    }

    return buffer;
};

// Whisper operates at 16 kHz mono. The browser records at its native rate
// (48 kHz on most hardware), so uploading the raw capture sent roughly three
// times the bytes for no benefit and made whisper.cpp resample on arrival.
OSA.WHISPER_SAMPLE_RATE = 16000;

OSA.audioBlobToWavBase64 = async function(blob) {
    const AudioContextCtor = window.AudioContext || window.webkitAudioContext;
    if (!AudioContextCtor) {
        throw new Error('Audio decoding is not supported in this browser.');
    }

    const audioContext = new AudioContextCtor();
    let decoded;
    try {
        const arrayBuffer = await blob.arrayBuffer();
        decoded = await audioContext.decodeAudioData(arrayBuffer.slice(0));
    } finally {
        if (typeof audioContext.close === 'function') {
            await audioContext.close().catch(() => {});
        }
    }

    const resampled = await OSA.resampleToWhisperRate(decoded);
    return OSA.arrayBufferToBase64(OSA.encodeAudioBufferToWav(resampled));
};

OSA.resampleToWhisperRate = async function(audioBuffer) {
    const target = OSA.WHISPER_SAMPLE_RATE;
    if (audioBuffer.sampleRate === target) return audioBuffer;

    const OfflineCtor = window.OfflineAudioContext || window.webkitOfflineAudioContext;
    if (!OfflineCtor) return audioBuffer;

    const frames = Math.max(
        1,
        Math.ceil(audioBuffer.duration * target),
    );

    try {
        // Mono out: the encoder downmixes anyway, and doing it here keeps the
        // resampler from doing three times the work.
        const offline = new OfflineCtor(1, frames, target);
        const source = offline.createBufferSource();
        source.buffer = audioBuffer;
        source.connect(offline.destination);
        source.start();
        return await offline.startRendering();
    } catch (err) {
        // Some browsers refuse unusual target rates. Uploading at the native
        // rate still works, it is just larger.
        console.warn('Resample to 16 kHz failed, sending native rate:', err);
        return audioBuffer;
    }
};

OSA.processLocalWhisperRecording = async function(blob) {
    let finalStatus = null;

    try {
        if (!blob || blob.size === 0) {
            throw new Error('No audio was captured from the microphone.');
        }

        const audioData = await OSA.audioBlobToWavBase64(blob);
        const response = await OSA.fetchWithAuth('/api/voice/transcribe', {
            method: 'POST',
            body: JSON.stringify({ audio_data: audioData })
        });
        const data = await response.json().catch(() => ({}));

        if (!response.ok) {
            throw new Error(data.error || `HTTP ${response.status}`);
        }

        const transcript = (data.text || '').trim();
        if (!transcript) {
            finalStatus = {
                message: 'Local Whisper did not hear any text. Try again and speak a little closer to the mic.',
                tone: 'error'
            };
            return;
        }

        OSA.setVoiceModeTranscript(transcript);
        OSA.applyTranscriptToInput(transcript);

        if (OSA.shouldAutoSendTranscript()) {
            const input = document.getElementById('message-input');
            if (input?.value.trim()) {
                OSA.sendMessage();
            }
        } else {
            finalStatus = {
                message: 'Transcript added to the chat box. Edit it if needed, then send.',
                tone: 'ready'
            };
        }
    } catch (error) {
        console.error('Local Whisper transcription failed:', error);
        finalStatus = {
            message: `Local Whisper failed: ${error.message}`,
            tone: 'error'
        };
    } finally {
        OSA.setIsTranscribing(false);
        OSA.updateMicButton();
        if (finalStatus) {
            OSA.setVoiceStatus(finalStatus.message, finalStatus.tone);
        }
    }
};

OSA.ensureLocalWhisperReady = async function() {
    const voiceConfig = OSA.normalizeVoiceConfig(OSA.getVoiceConfig());
    const selectedModel = voiceConfig?.whisper_model || null;
    const [statusResponse, installedResponse] = await Promise.all([
        OSA.fetchWithAuth('/api/voice/status'),
        OSA.fetchWithAuth('/api/voice/installed')
    ]);

    const status = await statusResponse.json().catch(() => ({}));
    const installed = await installedResponse.json().catch(() => ({}));

    if (!statusResponse.ok) {
        throw new Error(status.error || `Unable to read Local Whisper status (HTTP ${statusResponse.status}).`);
    }

    if (!installedResponse.ok) {
        throw new Error(installed.error || `Unable to read installed models (HTTP ${installedResponse.status}).`);
    }

    if (!status.whisper_installed) {
        throw new Error('Local Whisper runtime is not installed. Open Settings > Voice and install Local Whisper first.');
    }

    const installedModels = new Set((installed.whisper || []).map(model => model.id));
    if (selectedModel && !installedModels.has(selectedModel)) {
        throw new Error(`Selected Whisper model '${selectedModel}' is not downloaded. Open Settings > Voice and install it first.`);
    }

    if (!selectedModel && installedModels.size === 0) {
        throw new Error('No Whisper model is downloaded yet. Open Settings > Voice and install one first.');
    }
};

// Live input monitoring. Without this a muted or wrong-device microphone looks
// exactly like a working one until transcription comes back empty, which is the
// single most confusing failure in the voice path.
OSA.startLevelMonitor = function(stream) {
    OSA.stopLevelMonitor();
    const AudioCtx = window.AudioContext || window.webkitAudioContext;
    if (!AudioCtx) return;

    try {
        const ctx = new AudioCtx();
        const source = ctx.createMediaStreamSource(stream);
        const analyser = ctx.createAnalyser();
        analyser.fftSize = 512;
        analyser.smoothingTimeConstant = 0.6;
        source.connect(analyser);

        const buffer = new Uint8Array(analyser.fftSize);
        const startedAt = Date.now();
        let lastVoiceAt = Date.now();
        let sawVoice = false;

        OSA._levelMonitor = { ctx, analyser, raf: 0 };

        const tick = () => {
            if (!OSA._levelMonitor) return;
            analyser.getByteTimeDomainData(buffer);

            // RMS around the 128 midpoint of unsigned 8-bit PCM.
            let sum = 0;
            for (let i = 0; i < buffer.length; i += 1) {
                const v = (buffer[i] - 128) / 128;
                sum += v * v;
            }
            const rms = Math.sqrt(sum / buffer.length);
            const level = Math.min(1, rms * 4);

            OSA.renderMicLevel(level, Math.round((Date.now() - startedAt) / 1000));

            // Voice mode is hands-free by definition, so it always ends on
            // silence regardless of the setting.
            const autoStop = OSA.isVoiceModeOpen?.() || OSA.getVoiceConfig()?.silence_auto_stop;
            if (rms > 0.02) {
                lastVoiceAt = Date.now();
                sawVoice = true;
            } else if (autoStop && sawVoice && Date.now() - lastVoiceAt > 1800) {
                // Trailing silence after actual speech ends the utterance, so a
                // hands-free turn needs no second click.
                OSA.stopLocalWhisperRecording();
                return;
            }

            OSA._levelMonitor.raf = requestAnimationFrame(tick);
        };
        OSA._levelMonitor.raf = requestAnimationFrame(tick);
    } catch (err) {
        console.warn('Level monitor unavailable:', err);
    }
};

OSA.stopLevelMonitor = function() {
    const monitor = OSA._levelMonitor;
    OSA._levelMonitor = null;
    if (!monitor) return;
    if (monitor.raf) cancelAnimationFrame(monitor.raf);
    if (monitor.ctx && monitor.ctx.state !== 'closed') {
        monitor.ctx.close().catch(() => {});
    }
    OSA.renderMicLevel(null);
};

OSA.renderMicLevel = function(level, seconds) {
    // The voice-mode orb pulses off the same analyser, so a dead microphone
    // is visibly dead there too rather than just looking idle.
    const orb = document.getElementById('voice-mode-orb');
    if (orb) orb.style.setProperty('--mic-level', level === null ? '0' : String(level));

    const meter = document.getElementById('mic-meter');
    if (!meter) return;

    if (level === null) {
        meter.classList.add('hidden');
        meter.style.setProperty('--mic-level', '0');
        return;
    }

    meter.classList.remove('hidden');
    meter.style.setProperty('--mic-level', String(level));
    const timeEl = document.getElementById('mic-meter-time');
    if (timeEl && typeof seconds === 'number') {
        const mm = Math.floor(seconds / 60);
        const ss = String(seconds % 60).padStart(2, '0');
        timeEl.textContent = mm > 0 ? `${mm}:${ss}` : `0:${ss}`;
    }
};

// ---------------------------------------------------------------------------
// Streaming partial transcripts
//
// While recording, periodically transcribe everything captured so far and show
// it. Transcription then finishes at roughly the same moment speech does,
// instead of starting there.
//
// Two constraints shape this:
//   * MediaRecorder chunks after the first are not independently decodable —
//     they have no header — so each pass re-sends the growing prefix rather
//     than the newest slice.
//   * Only one request is ever in flight, and only when whisper-server is
//     installed. Against the CLI every partial would reload the whole model.
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// The spoken channel
//
// When speech is on the model emits a <speak> block first, then its normal
// written answer. The block is what gets read aloud; the written answer keeps
// its markdown, tables and code for the screen.
//
// Previously there was one channel for both, so making the reply speakable also
// stripped all formatting out of the chat.
// ---------------------------------------------------------------------------

OSA.SPEAK_BLOCK_RE = /<speak>([\s\S]*?)<\/speak>/i;
OSA.SPEAK_BLOCK_GLOBAL_RE = /<speak>[\s\S]*?<\/speak>\s*/gi;
// Matches an unterminated block while the response is still streaming.
OSA.SPEAK_OPEN_RE = /<speak>([\s\S]*)$/i;

/// The spoken text, or null when the model did not emit a block.
OSA.extractSpeakBlock = function(text) {
    const match = (text || '').match(OSA.SPEAK_BLOCK_RE);
    return match ? match[1].trim() : null;
};

/// The text to display: everything except the spoken block. Also hides a
/// partially-streamed opening tag so it never flashes into the chat.
OSA.stripSpeakBlock = function(text) {
    if (!text) return text;
    let out = text.replace(OSA.SPEAK_BLOCK_GLOBAL_RE, '');
    const open = out.match(OSA.SPEAK_OPEN_RE);
    if (open) out = out.slice(0, open.index);
    return out.replace(/^\s+/, '');
};

OSA.PARTIAL_INTERVAL_MS = 1400;

OSA.canStreamTranscripts = function() {
    return !!OSA._whisperServerAvailable;
};

OSA.refreshWhisperCapabilities = async function() {
    try {
        const res = await OSA.fetchWithAuth('/api/voice/status');
        if (!res.ok) return;
        const status = await res.json();
        OSA._whisperServerAvailable = !!status.whisper_server_available;
    } catch (err) {
        OSA._whisperServerAvailable = false;
    }
};

OSA.startPartialTranscription = function(mimeType) {
    OSA.stopPartialTranscription();
    if (!OSA.canStreamTranscripts()) return;

    // Partials from a previous utterance must never land in this one.
    const generation = (OSA._partialGeneration || 0) + 1;
    OSA._partialGeneration = generation;

    OSA._partialTimer = setInterval(async () => {
        if (generation !== OSA._partialGeneration) return;
        if (OSA._partialInFlight) return;
        if (!OSA.getIsRecording()) return;

        const chunks = OSA.getMediaChunks();
        if (!chunks.length) return;

        OSA._partialInFlight = true;
        try {
            const blob = new Blob(chunks, { type: mimeType || 'audio/webm' });
            const audioData = await OSA.audioBlobToWavBase64(blob);
            if (generation !== OSA._partialGeneration) return;

            const res = await OSA.fetchWithAuth('/api/voice/transcribe', {
                method: 'POST',
                body: JSON.stringify({ audio_data: audioData }),
            });
            if (!res.ok) return;

            const data = await res.json().catch(() => ({}));
            const text = (data.text || '').trim();
            if (!text || generation !== OSA._partialGeneration) return;
            if (!OSA.getIsRecording()) return;

            OSA.renderPartialTranscript(text);
        } catch (err) {
            // Partials are an optimisation; the final pass still runs.
            console.warn('Partial transcription failed:', err);
        } finally {
            OSA._partialInFlight = false;
        }
    }, OSA.PARTIAL_INTERVAL_MS);
};

OSA.stopPartialTranscription = function() {
    if (OSA._partialTimer) {
        clearInterval(OSA._partialTimer);
        OSA._partialTimer = null;
    }
    // Invalidate anything still in flight.
    OSA._partialGeneration = (OSA._partialGeneration || 0) + 1;
};

OSA.renderPartialTranscript = function(text) {
    OSA.setVoiceModeTranscript(text);
    OSA.setVoiceStatus(`Hearing: "${text}"`, 'recording');

    // Mirror into the composer so the inline mic shows progress too. Marked as
    // provisional so the final transcript replaces it rather than appending.
    const input = document.getElementById('message-input');
    if (input) {
        input.value = text;
        input.dataset.partialTranscript = '1';
    }
};

OSA.startLocalWhisperRecording = async function() {
    if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === 'undefined') {
        OSA.setVoiceStatus('Local Whisper needs browser microphone recording support before it can start.', 'error');
        return;
    }

    try {
        await OSA.ensureLocalWhisperReady();
        const stream = await navigator.mediaDevices.getUserMedia(OSA.buildAudioConstraints());
        const mimeType = [
            'audio/webm;codecs=opus',
            'audio/ogg;codecs=opus',
            'audio/webm',
            'audio/ogg'
        ].find(type => typeof MediaRecorder.isTypeSupported === 'function' && MediaRecorder.isTypeSupported(type));
        const recorder = mimeType ? new MediaRecorder(stream, { mimeType }) : new MediaRecorder(stream);

        OSA.setMediaStream(stream);
        OSA.setMediaRecorder(recorder);
        OSA.setMediaChunks([]);

        recorder.ondataavailable = (event) => {
            if (event.data && event.data.size > 0) {
                OSA.setMediaChunks([...OSA.getMediaChunks(), event.data]);
            }
        };

        recorder.onerror = (event) => {
            const message = event?.error?.message || 'Microphone recording failed.';
            console.error('MediaRecorder error:', event?.error || event);
            OSA.setIsRecording(false);
            OSA.setIsTranscribing(false);
            OSA.resetLocalRecorder();
            OSA.updateMicButton();
            OSA.setVoiceStatus(message, 'error');
        };

        recorder.onstop = () => {
            const chunks = OSA.getMediaChunks();
            const blob = new Blob(chunks, { type: recorder.mimeType || 'audio/webm' });
            OSA.resetLocalRecorder();
            OSA.processLocalWhisperRecording(blob);
        };

        // Timeslice: without it ondataavailable fires only on stop, so there
        // would be no audio to build a partial transcript from.
        recorder.start(OSA.canStreamTranscripts() ? 1000 : undefined);
        OSA.startPartialTranscription(recorder.mimeType);
        OSA.startLevelMonitor(stream);
        OSA.setIsRecording(true);
        OSA.setIsTranscribing(false);
        OSA.updateMicButton();
    } catch (error) {
        console.error('Failed to start local Whisper recording:', error);
        OSA.resetLocalRecorder();
        OSA.setIsRecording(false);
        OSA.setIsTranscribing(false);
        OSA.updateMicButton();
        OSA.setVoiceStatus(error.message || 'Unable to start Local Whisper recording.', 'error');
    }
};

OSA.stopLocalWhisperRecording = function() {
    const recorder = OSA.getMediaRecorder();
    if (!recorder || recorder.state === 'inactive') {
        OSA.setIsRecording(false);
        OSA.setIsTranscribing(false);
        OSA.updateMicButton();
        return;
    }

    OSA.setIsRecording(false);
    OSA.setIsTranscribing(true);
    OSA.updateMicButton();
    recorder.stop();
};

OSA.toggleRecording = function() {
    const voiceConfig = OSA.normalizeVoiceConfig(OSA.getVoiceConfig());
    let startError = null;

    if (!voiceConfig?.enabled) {
        alert('Voice features are disabled. Enable them in Settings.');
        return;
    }

    if (OSA.getIsTranscribing()) {
        return;
    }

    // Barge-in: never let our own playback bleed into the mic, and treat the
    // user reaching for the mic as "stop talking to me".
    if (!OSA.getIsRecording()) {
        OSA.cancelSpeechOutput();
    }

    const sttProvider = OSA.normalizeSttProvider(voiceConfig.stt_provider);
    if (sttProvider === 'whisper-local') {
        if (OSA.getIsRecording()) {
            OSA.stopLocalWhisperRecording();
        } else {
            OSA.startLocalWhisperRecording();
        }
        return;
    }

    let recognition = OSA.getRecognition();
    if (!recognition) {
        OSA.initSpeechRecognition();
        recognition = OSA.getRecognition();
        if (!recognition) {
            OSA.setVoiceStatus('Browser speech recognition is not supported here. Switch to Local Whisper or use Chrome or Edge.', 'error');
            return;
        }
    }

    if (OSA.getIsRecording()) {
        recognition.stop();
    } else {
        recognition.lang = voiceConfig?.language || 'en';
        try {
            recognition.start();
            OSA.setIsRecording(true);
        } catch (error) {
            console.error('Failed to start speech recognition:', error);
            startError = error;
        }
    }

    OSA.updateMicButton();
    if (startError) {
        OSA.setVoiceStatus(`Unable to start browser speech recognition: ${startError.message}`, 'error');
    }
};

OSA.updateMicButton = function() {
    const btn = document.getElementById('mic-btn');
    if (btn) {
        const label = btn.querySelector('.label');
        const isRecording = OSA.getIsRecording();
        const isTranscribing = OSA.getIsTranscribing();
        btn.classList.toggle('recording', isRecording);
        btn.classList.toggle('active', isRecording);
        btn.classList.toggle('busy', isTranscribing);
        btn.disabled = isTranscribing;
        btn.setAttribute('aria-pressed', isRecording ? 'true' : 'false');
        btn.setAttribute('aria-busy', isTranscribing ? 'true' : 'false');
        btn.setAttribute('aria-label', isTranscribing ? 'Transcribing voice input' : (isRecording ? 'Stop voice input' : 'Start voice input'));
        btn.title = isTranscribing ? 'Transcribing voice input' : (isRecording ? 'Stop voice input' : 'Start voice input');
        if (label) {
            label.textContent = isTranscribing ? 'Wait' : (isRecording ? 'Stop' : 'Talk');
        }
    }

    OSA.updateVoiceStatus();
    OSA.renderVoiceModeState();
};

// Tell the server whether replies for this session should be written to be
// heard. Best-effort: if it fails the frontend sanitiser still runs, we just
// lose the model-side shaping.
OSA.syncVoiceModeToSession = async function(enabled) {
    const session = OSA.getCurrentSession?.();
    if (!session?.id) return;

    // voice_mode lives on the session, so it has to be re-asserted for every
    // session, not just when the speaker button is clicked.
    if (session.metadata && session.metadata.voice_mode === enabled) return;
    if (session.metadata) session.metadata.voice_mode = enabled;

    try {
        await OSA.fetchWithAuth(`/api/sessions/${encodeURIComponent(session.id)}`, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ voice_mode: enabled }),
        });
    } catch (err) {
        console.warn('Failed to sync voice mode:', err);
    }
};

/// Makes the session's stored voice_mode match whether we are actually going to
/// speak the reply.
///
/// Previously this only ran from the speaker button, so turning speech on via
/// the auto_speak setting, opening voice mode when speech was already on, or
/// simply starting a new session all left the server believing voice was off —
/// and the model kept writing screen-shaped output full of units and symbols.
OSA.ensureVoiceModeSynced = async function() {
    await OSA.syncVoiceModeToSession(!!OSA.getTtsEnabled());
};

OSA.toggleTTS = function() {
    OSA.setTtsEnabled(!OSA.getTtsEnabled());
    OSA.updateTTSButton();
    OSA.syncVoiceModeToSession(OSA.getTtsEnabled());

    if (!OSA.getTtsEnabled()) {
        if (window.speechSynthesis.speaking) {
            window.speechSynthesis.cancel();
        }
        OSA.stopAudioPlayback();
        OSA.clearSpeechQueue();
    }
};

OSA.updateTTSButton = function() {
    const btn = document.getElementById('tts-btn');
    if (btn) {
        const label = btn.querySelector('.label');
        const ttsEnabled = OSA.getTtsEnabled();
        btn.classList.toggle('active', ttsEnabled);
        btn.setAttribute('aria-pressed', ttsEnabled ? 'true' : 'false');
        btn.title = ttsEnabled ? 'Disable speech' : 'Enable speech';
        if (label) {
            label.textContent = ttsEnabled ? 'Speak On' : 'Speak Off';
        }
    }
};

OSA.stopAudioPlayback = function() {
    const audio = OSA.getCurrentAudio();
    if (audio) {
        audio.onended = null;
        audio.pause();
        OSA.setCurrentAudio(null);
    }
    const url = OSA.getCurrentAudioUrl();
    if (url) {
        URL.revokeObjectURL(url);
        OSA.setCurrentAudioUrl(null);
    }
};

OSA.cancelSpeechOutput = function() {
    // Bumping the generation orphans any in-flight synthesis, whose completion
    // handler would otherwise clear the busy flag belonging to a newer run.
    OSA.bumpSpeechPlaybackGeneration();
    OSA.clearSpeechQueue();
    OSA._speechBusy = false;
    OSA.stopAudioPlayback();
    if (window.speechSynthesis && window.speechSynthesis.speaking) {
        window.speechSynthesis.cancel();
    }
    OSA.updateGlobalPlaybackBar();
};

OSA.isAudioPlaying = function() {
    return (OSA.getCurrentAudio() && !OSA.getCurrentAudio().paused) || window.speechSynthesis.speaking;
};

// Retained for callers outside the pipeline; the pump owns scheduling now.
OSA.processSpeechQueue = function() {
    OSA.pumpSpeechQueue();
};

// Backstop sanitiser. With voice-mode instructions in the system prompt the
// model should not be emitting most of this, but models ignore instructions and
// older sessions predate the change, so nothing here is safe to drop.
//
// Ordering matters: block constructs are removed before inline ones, and
// sentence-ending punctuation is inserted where structure used to carry the
// pause, otherwise the synthesizer runs headings into paragraphs.
OSA.cleanSpeechText = function(text) {
    return (text || '')
        // Fenced code, then 4-space indented code blocks.
        .replace(/```[\s\S]*?```/g, ' ')
        .replace(/^(?: {4}|\t).*$/gm, ' ')
        // Markdown tables: whole rows, before pipes get a chance to survive.
        .replace(/^\s*\|.*\|\s*$/gm, ' ')
        .replace(/^\s*\|?[\s:-]*\|[\s:|-]*$/gm, ' ')
        // Links and images: keep the label, drop the target.
        .replace(/!\[(.*?)\]\((.*?)\)/g, ' ')
        .replace(/\[(.*?)\]\((.*?)\)/g, '$1')
        // Bare URLs and email addresses.
        .replace(/\b(?:https?:\/\/|www\.)\S+/gi, ' the link on screen ')
        .replace(/\b[\w.+-]+@[\w-]+\.[\w.]+\b/g, ' the address on screen ')
        // Filesystem paths: Windows drive paths, UNC, and POSIX absolutes.
        .replace(/\b[A-Za-z]:\\[^\s"'`]+/g, ' the file on screen ')
        .replace(/\\\\[^\s"'`]+/g, ' the file on screen ')
        .replace(/(?:^|\s)~?\/[^\s"'`,;:]{2,}/g, ' the file on screen ')
        // Bare filenames with a code-ish extension.
        .replace(/\b[\w.-]+\.(?:rs|js|ts|tsx|jsx|py|toml|json|ya?ml|lock|md|html|css|sh|ps1|exe|db)\b/gi, ' the file on screen ')
        // Long hex runs: commit hashes, checksums, ids.
        .replace(/\b[0-9a-f]{7,}\b/gi, ' ')
        // Headings and blockquotes become sentences so the voice pauses.
        .replace(/^\s{0,3}#{1,6}\s+(.*)$/gm, '$1. ')
        .replace(/^\s{0,3}>\s?/gm, ' ')
        // List markers become sentence breaks rather than "dash dash dash".
        .replace(/^\s*[-*+]\s+/gm, ' ')
        .replace(/^\s*\d+[.)]\s+/gm, ' ')
        // Horizontal rules.
        .replace(/^\s*(?:-{3,}|\*{3,}|_{3,})\s*$/gm, ' ')
        // Units and symbols. Synthesizers read these literally or drop them:
        // "14°C" becomes "fourteen degree C", "19 km/h" becomes "K M slash H".
        .replace(/(\d)\s*°\s*C\b/gi, '$1 degrees')
        .replace(/(\d)\s*°\s*F\b/gi, '$1 degrees fahrenheit')
        .replace(/(\d)\s*°/g, '$1 degrees')
        .replace(/(\d)\s*%/g, '$1 percent')
        .replace(/(\d)\s*km\/h\b/gi, '$1 kilometres per hour')
        .replace(/(\d)\s*mph\b/gi, '$1 miles per hour')
        .replace(/(\d)\s*m\/s\b/gi, '$1 metres per second')
        .replace(/(\d)\s*(?:kg|kgs)\b/gi, '$1 kilograms')
        .replace(/(\d)\s*(?:km)\b/gi, '$1 kilometres')
        .replace(/(\d)\s*(?:cm)\b/gi, '$1 centimetres')
        .replace(/(\d)\s*(?:mm)\b/gi, '$1 millimetres')
        .replace(/(\d)\s*ms\b/gi, '$1 milliseconds')
        .replace(/(\d)\s*(GB|MB|KB)\b/g, (_, digit, unit) => {
            const words = { GB: 'gigabytes', MB: 'megabytes', KB: 'kilobytes' };
            return `${digit} ${words[unit]}`;
        })
        // A slash between words is read as "slash"; it almost always means "or".
        .replace(/\s+\/\s+/g, ', ')
        // Parentheses are a screen aside. Keep the words, drop the brackets,
        // and let the surrounding commas carry the pause.
        .replace(/\s*\(([^)]{1,120})\)\s*/g, ', $1, ')
        // Dashes used as punctuation read as nothing or as "dash".
        .replace(/\s*[—–]\s*/g, ', ')

        // Inline code and remaining emphasis markers.
        .replace(/`([^`]+)`/g, '$1')
        .replace(/[*_~]/g, ' ')
        .replace(/[{}|<>]/g, ' ')
        // Emoji and pictographs: most engines read these by Unicode name.
        .replace(/[\u{1F000}-\u{1FAFF}\u{2600}-\u{27BF}\u{2190}-\u{21FF}\u{2B00}-\u{2BFF}\u{FE0F}\u{200D}]/gu, ' ')
        // A line that ended without punctuation was a structural break (list
        // item, heading, row). Restore it as a sentence so the voice pauses
        // instead of running the next line straight on.
        .replace(/([^\s.!?:;,])[ \t]*\r?\n+/g, '$1. ')
        // Tidy the punctuation the substitutions left behind.
        .replace(/\s+([.,!?;:])/g, '$1')
        .replace(/([.!?])\s*(?=[.!?])/g, '')
        .replace(/\s+/g, ' ')
        .trim();
};

OSA.stripMachineReadableSpeech = function(text) {
    return (text || '')
        .replace(/\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi, ' ')
        .replace(/\b(?:session|task|tool call|checkpoint|workspace)\s+id\b[:#-]*\s*[A-Za-z0-9_-]+/gi, ' ')
        .replace(/\b[a-z_]+_id\b\s*[:=]\s*[A-Za-z0-9_-]+/gi, ' ')
        .replace(/\btool[_ -]?call[_ -]?id\b[:#-]*\s*[A-Za-z0-9_-]+/gi, ' ')
        .replace(/\bid\s*[:=]\s*[A-Za-z0-9_-]+/gi, ' ')
        .replace(/\b[a-f0-9]{24,}\b/gi, ' ')
        .replace(/\b\d{6,}\b/g, ' ')
        .replace(/\s+/g, ' ')
        .trim();
};

OSA.sanitizeSpeechText = function(text) {
    // The id-stripping pass runs last and can leave stranded punctuation behind
    // where it removed a whole clause, so tidy once more after both passes.
    return OSA.stripMachineReadableSpeech(OSA.cleanSpeechText(text))
        .replace(/\s+([.,!?;:])/g, '$1')
        // Unwrapping brackets and dashes into commas can strand a comma against
        // other punctuation: "(… WA):" becomes "… WA,:". Collapse those runs to
        // the strongest mark so the pause is right.
        .replace(/,\s*([.!?;:])/g, '$1')
        .replace(/(?:,\s*){2,}/g, ', ')
        .replace(/([.!?])(?:\s*[.,;:])+/g, '$1')
        .replace(/\s+/g, ' ')
        .trim();
};

OSA.summarizeForSpeech = function(text, maxLen = 320) {
    const clean = OSA.sanitizeSpeechText(text);
    if (!clean) return '';

    const parts = clean.match(/[^.!?]+[.!?]?/g) || [clean];
    let summary = '';
    for (const part of parts) {
        const next = summary ? `${summary} ${part.trim()}` : part.trim();
        if (next.length > maxLen) break;
        summary = next;
        if (summary.length > maxLen * 0.65 && /[.!?]$/.test(summary)) {
            break;
        }
    }

    return (summary || clean.slice(0, maxLen)).replace(/[{}]/g, '').trim();
};

// How much of a reply we are willing to read out. The model is instructed to
// stay well inside this when voice mode is on; this only bites when it doesn't.
OSA.SPEECH_MAX_LEN = 700;

OSA.prepareSpeechText = function(text, isRoleplay) {
    if (!text) return '';

    if (isRoleplay) {
        const quotes = text.match(/"[^"]+"/g);
        if (quotes && quotes.length > 0) {
            return quotes.join(' ').replace(/"/g, '');
        }
        // A custom persona that doesn't use quotation marks used to produce
        // total silence with no explanation. Fall through to the normal path.
    }

    const clean = OSA.sanitizeSpeechText(text);
    if (!clean) return '';

    const spoken = OSA.summarizeForSpeech(text, OSA.SPEECH_MAX_LEN);
    if (!spoken) return '';

    // Truncation used to be silent, so a voice user could not tell a short
    // answer from a decapitated one. Say so.
    if (spoken.length < clean.length * 0.9) {
        return `${spoken} There's more on screen.`;
    }
    return spoken;
};

OSA.summarizeToolArguments = function(args) {
    if (!args || typeof args !== 'object') return '';

    const pieces = [];
    for (const [key, value] of Object.entries(args).slice(0, 3)) {
        if (/(_id|^id$|session|tool_call|checkpoint)/i.test(key)) {
            continue;
        }
        const label = key.replace(/_/g, ' ');
        if (typeof value === 'string') {
            const shortValue = OSA.summarizeForSpeech(value, 60);
            if (shortValue) pieces.push(`${label}: ${shortValue}`);
        } else if (typeof value === 'number' || typeof value === 'boolean') {
            if (typeof value === 'number' && value > 99999) {
                continue;
            }
            pieces.push(`${label}: ${value}`);
        } else if (Array.isArray(value)) {
            const humanItems = value
                .map(item => typeof item === 'string' ? OSA.sanitizeSpeechText(item) : '')
                .filter(Boolean)
                .slice(0, 2);
            if (humanItems.length) {
                pieces.push(`${label}: ${humanItems.join('. ')}`);
            } else if (value.every(item => typeof item === 'number' && item > 99999)) {
                continue;
            } else {
                pieces.push(`${label}: ${value.length} items`);
            }
        } else if (value && typeof value === 'object') {
            pieces.push(`${label}: provided`);
        }
    }

    return pieces.join('. ');
};

OSA.speakToolStart = function(event) {
    const ttsEnabled = OSA.getTtsEnabled();
    const voiceConfig = OSA.getVoiceConfig();
    if (!ttsEnabled || !voiceConfig?.enabled) return;
    if (!voiceConfig?.speak_tool_progress) return;
    const toolName = (event.tool_name || 'tool').replace(/[_-]/g, ' ');
    const args = OSA.summarizeToolArguments(event.arguments);
    const text = args ? `Running ${toolName}. ${args}.` : `Running ${toolName}.`;
    OSA.speakText(text, { interrupt: false });
};

OSA.speakToolComplete = function(event) {
    const ttsEnabled = OSA.getTtsEnabled();
    const voiceConfig = OSA.getVoiceConfig();
    if (!ttsEnabled || !voiceConfig?.enabled) return;
    if (!voiceConfig?.speak_tool_progress) return;
    const toolName = (event.tool_name || 'tool').replace(/[_-]/g, ' ');
    const text = event.success ? `Finished ${toolName}.` : `${toolName} failed.`;
    OSA.speakText(text, { interrupt: false });
};

// Serialised speech playback.
//
// Busy state is an explicit flag rather than `isAudioPlaying()`. Piper
// synthesises over HTTP, and during that request no audio element exists yet,
// so `isAudioPlaying()` reads false — which meant a second streamed sentence
// would start its own request instead of queueing, and both would then call
// stopAudioPlayback() on each other. With streaming TTS emitting a sentence a
// second, that produced constant overlap and clipped words.
OSA.isSpeechBusy = function() {
    return !!OSA._speechBusy;
};

/// True from the moment an utterance is claimed until playback ends, including
/// the Piper synthesis request when no audio element exists yet. UI state must
/// use this rather than isAudioPlaying(), which goes false mid-pipeline.
OSA.isSpeaking = function() {
    return OSA.isSpeechBusy() || OSA.isAudioPlaying();
};

OSA.speakText = function(text, options = {}) {
    const ttsEnabled = OSA.getTtsEnabled();
    const voiceConfig = OSA.getVoiceConfig();
    if (!ttsEnabled || !voiceConfig?.enabled) return;

    const payload = OSA.sanitizeSpeechText(text).slice(0, 1000);
    if (!payload) return;

    if (options.interrupt !== false) {
        // An explicit interrupt drops anything pending and stops current audio.
        OSA.cancelSpeechOutput();
    }

    OSA.pushToSpeechQueue(payload);
    OSA.pumpSpeechQueue();
};

/// Plays the next utterance if nothing is currently being spoken or fetched.
OSA.pumpSpeechQueue = function() {
    const voiceConfig = OSA.getVoiceConfig();
    if (!OSA.getTtsEnabled() || !voiceConfig?.enabled) {
        OSA.clearSpeechQueue();
        return;
    }
    if (OSA._speechBusy) return;

    const queue = OSA.getSpeechQueue();
    if (!queue.length) {
        OSA.updateGlobalPlaybackBar();
        return;
    }

    const payload = queue.shift();
    // Claimed synchronously, before any await, so a segment arriving mid-fetch
    // queues instead of racing.
    OSA._speechBusy = true;
    OSA.appendVoiceModeReply(payload);

    const generation = OSA.getSpeechPlaybackGeneration();
    const finish = () => {
        // A cancel bumps the generation; that run no longer owns the pipeline.
        if (generation !== OSA.getSpeechPlaybackGeneration()) return;
        OSA._speechBusy = false;
        OSA.pumpSpeechQueue();
    };

    if (voiceConfig?.tts_provider === 'piper-local') {
        fetch('/api/tts/synthesize', {
            method: 'POST',
            headers: {
                'Authorization': `Bearer ${OSA.getToken()}`,
                'Content-Type': 'application/json'
            },
            body: JSON.stringify({ text: payload })
        })
        .then(res => {
            if (!res.ok) throw new Error('TTS failed');
            return res.blob();
        })
        .then(blob => {
            if (generation !== OSA.getSpeechPlaybackGeneration()) return;

            const url = URL.createObjectURL(blob);
            OSA.setCurrentAudioUrl(url);
            const audio = new Audio(url);
            OSA.setCurrentAudio(audio);
            audio.playbackRate = voiceConfig?.voice_speed || 1.0;
            audio.onended = finish;
            audio.onerror = finish;
            audio.play().catch(e => {
                console.error('Audio play failed:', e);
                finish();
            });
            OSA.updateGlobalPlaybackBar();
        })
        .catch(e => {
            console.error('Piper TTS error:', e);
            finish();
        });
    } else {
        const utterance = new SpeechSynthesisUtterance(payload);
        utterance.lang = voiceConfig?.language || 'en';
        utterance.rate = voiceConfig?.voice_speed || 1.0;
        utterance.onend = finish;
        utterance.onerror = finish;
        window.speechSynthesis.speak(utterance);
        OSA.updateGlobalPlaybackBar();
    }
};

// Sentence-at-a-time speech off the token stream.
//
// Speech used to fire once from the turn-completion handler, so time to first
// audio was the full turn latency plus a single blocking synthesis of the whole
// reply. Speaking each finished sentence as it arrives collapses that to
// roughly the first sentence, and pipelines synthesis with generation.
OSA.resetSpeechStream = function() {
    OSA._speechStreamBuffer = '';
    OSA._speechStreamSpoken = 0;
    OSA._speechBlockComplete = false;
};

OSA.feedSpeechStream = function(chunk) {
    if (!chunk) return;
    const voiceConfig = OSA.getVoiceConfig();
    if (!OSA.getTtsEnabled() || !voiceConfig?.enabled) return;
    // Roleplay speaks only quoted dialogue, which cannot be decided per
    // sentence; that path stays on the end-of-turn handler.
    if (OSA.getActivePersona?.()?.id === 'custom') return;

    OSA._speechStreamBuffer = (OSA._speechStreamBuffer || '') + chunk;

    // Speak only the <speak> block when the model provided one. Everything
    // after it is written for the screen and must never be read aloud.
    const raw = OSA._speechStreamBuffer;
    let buffer;
    if (/<speak>/i.test(raw)) {
        const closed = OSA.extractSpeakBlock(raw);
        if (closed !== null) {
            buffer = closed;
            OSA._speechBlockComplete = true;
        } else {
            // Still streaming inside the block: speak what has arrived so far.
            buffer = (raw.match(OSA.SPEAK_OPEN_RE) || [null, ''])[1];
        }
    } else if (OSA._speechBlockComplete) {
        // Block already finished; the rest of the response is screen-only.
        return;
    } else {
        buffer = raw;
    }
    if (!buffer) return;

    // Only emit up to the last sentence terminator, so we never speak half a
    // sentence. Code fences are held back entirely until they close, otherwise
    // a stray '.' inside code would be spoken mid-block.
    const openFences = (buffer.match(/```/g) || []).length;
    if (openFences % 2 === 1) return;

    // A finished <speak> block is a complete unit, so flush all of it. Waiting
    // for a terminator *followed by whitespace* would never fire: the block
    // ends "...weekend.</speak>" with nothing after the full stop.
    const lastStop = OSA._speechBlockComplete
        ? buffer.length - 1
        : Math.max(
            buffer.lastIndexOf('. '),
            buffer.lastIndexOf('! '),
            buffer.lastIndexOf('? '),
            buffer.lastIndexOf('.\n'),
            buffer.lastIndexOf('!\n'),
            buffer.lastIndexOf('?\n'),
        );
    if (lastStop <= OSA._speechStreamSpoken) return;

    const segment = buffer.slice(OSA._speechStreamSpoken, lastStop + 1);
    OSA._speechStreamSpoken = lastStop + 1;

    const spoken = OSA.sanitizeSpeechText(segment);
    if (spoken) {
        // Queued, not interrupting: successive sentences must play in order.
        OSA.speakText(spoken, { interrupt: false });
    }
};

/// True when the streaming path already voiced this turn, so the end-of-turn
/// handler does not repeat it.
OSA.speechStreamHandledTurn = function() {
    return (OSA._speechStreamSpoken || 0) > 0;
};

// Speaks one specific message on demand, independent of whether TTS is on for
// the session. Speech used to be fire-and-forget: miss a sentence and your only
// option was to go back and read it.
OSA.speakMessageElement = function(button) {
    const messageEl = button.closest('.message');
    if (!messageEl) return;

    const contentEl = messageEl.querySelector('.message-content');
    const raw = contentEl?.dataset.rawText || contentEl?.innerText || '';
    const text = OSA.sanitizeSpeechText(raw);
    if (!text) return;

    // Clicking the button of the message currently playing acts as stop.
    if (OSA._speakingMessageEl === messageEl && OSA.isSpeaking()) {
        OSA.cancelSpeechOutput();
        OSA.setSpeakingMessage(null);
        return;
    }

    OSA.cancelSpeechOutput();
    OSA.setSpeakingMessage(messageEl);

    // Replay is explicit, so honour it even when auto-speak is off. speakText
    // gates on the TTS toggle, so lift it for the duration of this utterance.
    const wasEnabled = OSA.getTtsEnabled();
    if (!wasEnabled) OSA.setTtsEnabled(true);
    OSA.speakText(text, { interrupt: true });
    if (!wasEnabled) {
        // Restore the toggle without cancelling what we just queued.
        OSA.setTtsEnabled(false);
    }
};

OSA.setSpeakingMessage = function(messageEl) {
    if (OSA._speakingMessageEl && OSA._speakingMessageEl !== messageEl) {
        OSA._speakingMessageEl.classList.remove('speaking');
    }
    OSA._speakingMessageEl = messageEl;
    if (messageEl) messageEl.classList.add('speaking');
    OSA.updateGlobalPlaybackBar();
};

// Playback end has no single callback we can hook (browser speech and Piper
// take different paths, and queued utterances chain), so the bar and the
// per-message highlight are kept in sync by polling only while audio is live.
OSA.updateGlobalPlaybackBar = function() {
    const bar = document.getElementById('playback-bar');
    const playing = OSA.isSpeaking();

    if (bar) bar.classList.toggle('hidden', !playing);
    OSA.renderVoiceModeState();

    if (playing && !OSA._playbackPoll) {
        OSA._playbackPoll = setInterval(() => {
            if (OSA.isSpeaking()) return;
            clearInterval(OSA._playbackPoll);
            OSA._playbackPoll = null;
            if (OSA._speakingMessageEl) {
                OSA._speakingMessageEl.classList.remove('speaking');
                OSA._speakingMessageEl = null;
            }
            const el = document.getElementById('playback-bar');
            if (el) el.classList.add('hidden');
            OSA.renderVoiceModeState();
        }, 250);
    }
};

OSA.stopAllPlayback = function() {
    OSA.cancelSpeechOutput();
    OSA.setSpeakingMessage(null);
};

OSA.updateVoiceButtons = function() {
    const controls = document.getElementById('voice-controls');
    const voiceConfig = OSA.getVoiceConfig();
    if (controls) {
        controls.classList.toggle('hidden', !voiceConfig?.enabled);
    }
    OSA.updateMicButton();
    OSA.updateTTSButton();
};

// ---------------------------------------------------------------------------
// Voice mode
//
// A view for when the user is not looking closely at the screen: one large
// target, state readable at a glance, and the transcript as the primary
// element rather than a status string under a text box.
// ---------------------------------------------------------------------------

OSA.isVoiceModeOpen = function() {
    const el = document.getElementById('voice-mode');
    return !!el && !el.classList.contains('hidden');
};

OSA.openVoiceMode = async function() {
    const el = document.getElementById('voice-mode');
    if (!el) return;

    if (!OSA.getVoiceConfig()?.enabled) {
        OSA.setVoiceStatus('Voice is disabled. Enable it in Settings first.', 'error');
        return;
    }

    el.classList.remove('hidden');
    document.body.classList.add('voice-mode-active');

    // Speaking is the point of this view, so turn it on when entering.
    OSA._ttsBeforeVoiceMode = OSA.getTtsEnabled();
    if (!OSA.getTtsEnabled()) {
        OSA.toggleTTS();
    } else {
        // Already on, so toggleTTS will not fire and would not sync.
        await OSA.ensureVoiceModeSynced();
    }

    await OSA.populateDevicePicker();
    OSA.renderVoiceModeState();
};

OSA.closeVoiceMode = function() {
    const el = document.getElementById('voice-mode');
    if (!el) return;

    if (OSA.getIsRecording()) {
        OSA.toggleRecording();
    }
    OSA.stopPartialTranscription?.();
    OSA.stopAllPlayback();

    // Any text still in the composer from a partial transcript is provisional
    // and will never be superseded now that streaming has stopped. Drop the
    // marker so it behaves like something the user typed, rather than being
    // silently replaced by the next dictation.
    const input = document.getElementById('message-input');
    if (input) delete input.dataset.partialTranscript;

    // Clear the panels, otherwise reopening shows the previous conversation's
    // transcript and reply as though they were current.
    OSA.setVoiceModeTranscript('');
    OSA.resetVoiceModeReply();

    el.classList.add('hidden');
    document.body.classList.remove('voice-mode-active');

    // Leave TTS as the user found it.
    if (OSA._ttsBeforeVoiceMode === false && OSA.getTtsEnabled()) {
        OSA.toggleTTS();
    }
};

OSA.populateDevicePicker = async function() {
    const select = document.getElementById('voice-mode-device');
    if (!select) return;

    let devices = await OSA.listInputDevices();

    // Labels stay blank until the page has held a mic stream once. Ask for one
    // and release it immediately, so the picker shows real device names.
    if (devices.length && !devices.some(d => d.label)) {
        try {
            const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
            stream.getTracks().forEach(track => track.stop());
            devices = await OSA.listInputDevices();
        } catch (err) {
            console.warn('Microphone permission not granted; device names hidden:', err);
        }
    }

    const preferred = OSA.getPreferredInputDevice();
    select.innerHTML = '';

    const auto = document.createElement('option');
    auto.value = '';
    auto.textContent = devices.length ? 'System default microphone' : 'No microphone found';
    select.appendChild(auto);

    devices.forEach((device, index) => {
        const option = document.createElement('option');
        option.value = device.deviceId;
        option.textContent = device.label || `Microphone ${index + 1}`;
        select.appendChild(option);
    });

    select.value = devices.some(d => d.deviceId === preferred) ? preferred : '';
    select.onchange = () => {
        OSA.setPreferredInputDevice(select.value);
        // Re-acquire on the next recording; an active one keeps the old device.
        OSA.setVoiceStatus('Microphone updated.', 'ready');
    };
};

/// Single source of truth for what voice mode displays. Called from the mic
/// button, the transcriber, and playback so all four states stay consistent.
OSA.renderVoiceModeState = function() {
    if (!OSA.isVoiceModeOpen()) return;

    const orb = document.getElementById('voice-mode-orb');
    const label = document.getElementById('voice-mode-state');
    if (!orb || !label) return;

    let state = 'idle';
    let text = 'Tap to speak';

    // Order matters. Speaking is checked before thinking because streaming TTS
    // starts talking while the turn is still running: the agent is processing
    // and playing audio at the same time, and "Speaking" is what the user is
    // actually experiencing.
    if (OSA.getIsRecording()) {
        state = 'listening';
        text = 'Listening';
    } else if (OSA.getIsTranscribing()) {
        state = 'thinking';
        text = 'Transcribing';
    } else if (OSA.isSpeaking()) {
        state = 'speaking';
        text = 'Speaking';
    } else if (OSA.isAgentProcessing()) {
        state = 'thinking';
        text = 'Thinking';
    }

    orb.dataset.state = state;
    label.textContent = text;
    label.dataset.state = state;
};

OSA.setVoiceModeTranscript = function(text) {
    const el = document.getElementById('voice-mode-transcript');
    if (el) el.textContent = text || '';
};

OSA.setVoiceModeReply = function(text) {
    const el = document.getElementById('voice-mode-reply');
    if (!el) return;
    // Show what is being spoken, not the raw markdown behind it.
    OSA._voiceModeReply = OSA.sanitizeSpeechText(text || '');
    el.textContent = OSA._voiceModeReply.slice(0, 600);
};

/// Streaming speaks a sentence at a time, so the panel has to accumulate:
/// assigning each segment left only the most recent sentence on screen.
OSA.appendVoiceModeReply = function(text) {
    const el = document.getElementById('voice-mode-reply');
    if (!el || !text) return;
    const existing = OSA._voiceModeReply || '';
    OSA._voiceModeReply = existing ? `${existing} ${text}` : text;
    el.textContent = OSA._voiceModeReply.slice(-600);
    el.scrollTop = el.scrollHeight;
};

OSA.resetVoiceModeReply = function() {
    OSA._voiceModeReply = '';
    const el = document.getElementById('voice-mode-reply');
    if (el) el.textContent = '';
};

// Push-to-talk. Hold space while not typing to record, release to finish.
// Ignored whenever focus is in a text field so it never eats a real space.
OSA.initPushToTalk = function() {
    let held = false;

    const isTypingTarget = (el) => {
        if (!el) return false;
        const tag = el.tagName;
        return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable;
    };

    document.addEventListener('keydown', (event) => {
        if (event.code !== 'Space' || event.repeat) return;
        if (event.ctrlKey || event.metaKey || event.altKey) return;
        if (isTypingTarget(document.activeElement)) return;
        if (!OSA.getVoiceConfig()?.enabled) return;
        if (OSA.getIsRecording() || OSA.getIsTranscribing()) return;

        event.preventDefault();
        held = true;
        OSA.toggleRecording();
    });

    document.addEventListener('keyup', (event) => {
        if (event.code !== 'Space' || !held) return;
        held = false;
        if (OSA.getIsRecording()) {
            event.preventDefault();
            OSA.toggleRecording();
        }
    });

    // A dropped keyup (alt-tab mid-hold) would otherwise record forever.
    window.addEventListener('blur', () => {
        if (held && OSA.getIsRecording()) {
            held = false;
            OSA.toggleRecording();
        }
    });
};

window.toggleRecording = OSA.toggleRecording;
window.toggleTTS = OSA.toggleTTS;
