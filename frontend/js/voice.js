window.OSA = window.OSA || {};

OSA.initVoice = function() {
    const config = OSA.getCachedConfig();
    if (!config?.voice) {
        fetch('/api/config', {
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        })
        .then(res => res.json())
        .then(cfg => {
            OSA.setVoiceConfig(cfg.voice);
            if (cfg.voice?.enabled) {
                OSA.initSpeechRecognition();
                OSA.setTtsEnabled(cfg.voice.auto_speak);
                OSA.updateVoiceButtons();
            }
        })
        .catch(err => console.error('Failed to load voice config:', err));
        return;
    }
    
    OSA.setVoiceConfig(config.voice);
    if (config.voice?.enabled) {
        OSA.initSpeechRecognition();
        OSA.setTtsEnabled(config.voice.auto_speak);
        OSA.updateVoiceButtons();
    }
};

OSA.initSpeechRecognition = function() {
    if (!('webkitSpeechRecognition' in window) && !('SpeechRecognition' in window)) {
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
    };
    
    OSA.setRecognition(recognition);
};

OSA.toggleRecording = function() {
    const voiceConfig = OSA.getVoiceConfig();
    if (!voiceConfig?.enabled) {
        alert('Voice features are disabled. Enable them in Settings.');
        return;
    }
    
    let recognition = OSA.getRecognition();
    if (!recognition) {
        OSA.initSpeechRecognition();
        recognition = OSA.getRecognition();
        if (!recognition) {
            alert('Speech recognition not supported. Try Chrome or Edge.');
            return;
        }
    }
    
    if (OSA.getIsRecording()) {
        recognition.stop();
        OSA.setIsRecording(false);
    } else {
        recognition.lang = voiceConfig?.language || 'en';
        recognition.start();
        OSA.setIsRecording(true);
    }
    
    OSA.updateMicButton();
};

OSA.updateMicButton = function() {
    const btn = document.getElementById('mic-btn');
    if (btn) {
        const label = btn.querySelector('.label');
        const isRecording = OSA.getIsRecording();
        btn.classList.toggle('recording', isRecording);
        btn.classList.toggle('active', isRecording);
        btn.setAttribute('aria-pressed', isRecording ? 'true' : 'false');
        btn.title = isRecording ? 'Stop voice input' : 'Start voice input';
        if (label) {
            label.textContent = isRecording ? 'Stop' : 'Talk';
        }
    }
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
