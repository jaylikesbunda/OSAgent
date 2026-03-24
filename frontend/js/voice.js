window.OSA = window.OSA || {};

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
    OSA.setMediaRecorder(null);
    OSA.setMediaChunks([]);
    OSA.stopLocalMediaStream();
};

OSA.setVoiceStatus = function(message, tone = 'idle') {
    OSA.setVoiceStatusMessage(message || '');
    const status = document.getElementById('voice-status');
    if (!status) return;

    if (!message) {
        status.textContent = '';
        status.classList.add('hidden');
        status.dataset.state = 'hidden';
        return;
    }

    status.textContent = message;
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
        const transcript = Array.from(event.results)
            .map(result => result[0].transcript)
            .join('');
        
        document.getElementById('message-input').value = transcript;
    };
    
    recognition.onend = () => {
        if (OSA.getIsRecording()) {
            OSA.setIsRecording(false);
            OSA.updateMicButton();

            if (OSA.getVoiceConfig()?.auto_send) {
                const input = document.getElementById('message-input');
                if (input.value.trim()) {
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

OSA.applyTranscriptToInput = function(text) {
    const input = document.getElementById('message-input');
    const transcript = (text || '').trim();
    if (!input || !transcript) return;

    const current = input.value.trim();
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

OSA.audioBlobToWavBase64 = async function(blob) {
    const AudioContextCtor = window.AudioContext || window.webkitAudioContext;
    if (!AudioContextCtor) {
        throw new Error('Audio decoding is not supported in this browser.');
    }

    const audioContext = new AudioContextCtor();
    try {
        const arrayBuffer = await blob.arrayBuffer();
        const audioBuffer = await audioContext.decodeAudioData(arrayBuffer.slice(0));
        const wavBuffer = OSA.encodeAudioBufferToWav(audioBuffer);
        return OSA.arrayBufferToBase64(wavBuffer);
    } finally {
        if (typeof audioContext.close === 'function') {
            await audioContext.close().catch(() => {});
        }
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

        OSA.applyTranscriptToInput(transcript);

        if (OSA.getVoiceConfig()?.auto_send) {
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

OSA.startLocalWhisperRecording = async function() {
    if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === 'undefined') {
        OSA.setVoiceStatus('Local Whisper needs browser microphone recording support before it can start.', 'error');
        return;
    }

    try {
        await OSA.ensureLocalWhisperReady();
        const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
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

        recorder.start();
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
};

OSA.toggleTTS = function() {
    OSA.setTtsEnabled(!OSA.getTtsEnabled());
    OSA.updateTTSButton();
    
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
        audio.pause();
        OSA.setCurrentAudio(null);
    }
    const url = OSA.getCurrentAudioUrl();
    if (url) {
        URL.revokeObjectURL(url);
        OSA.setCurrentAudioUrl(null);
    }
};

OSA.isAudioPlaying = function() {
    return (OSA.getCurrentAudio() && !OSA.getCurrentAudio().paused) || window.speechSynthesis.speaking;
};

OSA.processSpeechQueue = function() {
    const ttsEnabled = OSA.getTtsEnabled();
    const voiceConfig = OSA.getVoiceConfig();
    const queue = OSA.getSpeechQueue();
    
    if (!ttsEnabled || !voiceConfig?.enabled) {
        OSA.clearSpeechQueue();
        return;
    }
    if (OSA.isAudioPlaying() || queue.length === 0) {
        return;
    }

    const next = queue.shift();
    OSA.speakText(next, { interrupt: true, fromQueue: true });
};

OSA.cleanSpeechText = function(text) {
    return (text || '')
        .replace(/```[\s\S]*?```/g, ' ')
        .replace(/`([^`]+)`/g, '$1')
        .replace(/\[(.*?)\]\((.*?)\)/g, '$1')
        .replace(/[{}]/g, ' ')
        .replace(/[*_~>#]/g, ' ')
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
    return OSA.stripMachineReadableSpeech(OSA.cleanSpeechText(text));
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

OSA.prepareSpeechText = function(text, isRoleplay) {
    if (!text) return '';
    if (isRoleplay) {
        const quotes = text.match(/"[^"]+"/g);
        if (quotes && quotes.length > 0) {
            return quotes.join(' ').replace(/"/g, '');
        }
        return '';
    }
    return OSA.summarizeForSpeech(text, 280);
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
    const toolName = (event.tool_name || 'tool').replace(/[_-]/g, ' ');
    const args = OSA.summarizeToolArguments(event.arguments);
    const text = args ? `Running ${toolName}. ${args}.` : `Running ${toolName}.`;
    OSA.speakText(text, { interrupt: false });
};

OSA.speakToolComplete = function(event) {
    const ttsEnabled = OSA.getTtsEnabled();
    const voiceConfig = OSA.getVoiceConfig();
    if (!ttsEnabled || !voiceConfig?.enabled) return;
    const toolName = (event.tool_name || 'tool').replace(/[_-]/g, ' ');
    const text = event.success ? `Finished ${toolName}.` : `${toolName} failed.`;
    OSA.speakText(text, { interrupt: false });
};

OSA.speakText = function(text, options = {}) {
    const ttsEnabled = OSA.getTtsEnabled();
    const voiceConfig = OSA.getVoiceConfig();
    if (!ttsEnabled || !voiceConfig?.enabled) return;

    const payload = OSA.sanitizeSpeechText(text).slice(0, 1000);
    if (!payload) return;

    const interrupt = options.interrupt !== false;
    const fromQueue = options.fromQueue === true;
    if (!interrupt && OSA.isAudioPlaying()) {
        OSA.pushToSpeechQueue(payload);
        return;
    }
    
    if (interrupt && window.speechSynthesis.speaking) {
        window.speechSynthesis.cancel();
    }
    if (interrupt) {
        OSA.stopAudioPlayback();
        if (!fromQueue) {
            OSA.clearSpeechQueue();
        }
    }
    
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
            OSA.stopAudioPlayback();
            const url = URL.createObjectURL(blob);
            OSA.setCurrentAudioUrl(url);
            const audio = new Audio(url);
            OSA.setCurrentAudio(audio);
            audio.playbackRate = voiceConfig?.voice_speed || 1.0;
            audio.play().catch(e => console.error('Audio play failed:', e));
            audio.onended = () => {
                OSA.stopAudioPlayback();
                OSA.processSpeechQueue();
            };
        })
        .catch(e => {
            console.error('Piper TTS error:', e);
            OSA.processSpeechQueue();
        });
    } else {
        const utterance = new SpeechSynthesisUtterance(payload);
        utterance.lang = voiceConfig?.language || 'en';
        utterance.rate = voiceConfig?.voice_speed || 1.0;
        utterance.onend = () => OSA.processSpeechQueue();
        utterance.onerror = () => OSA.processSpeechQueue();
        window.speechSynthesis.speak(utterance);
    }
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

window.toggleRecording = OSA.toggleRecording;
window.toggleTTS = OSA.toggleTTS;
