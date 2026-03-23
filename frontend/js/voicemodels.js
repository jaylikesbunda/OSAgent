window.OSA = window.OSA || {};

OSA.voiceModels = {
    whisper: [],
    piper: [],
    installed: { whisper: [], piper: [] },
    progress: {},
    selectedWhisper: null,
    selectedPiper: null,
    piperLanguage: 'en',
    eventSource: null
};

OSA.loadVoiceModels = async function() {
    try {
        const [modelsRes, installedRes, configRes] = await Promise.all([
            OSA.getJson('/api/voice/models'),
            OSA.getJson('/api/voice/installed'),
            fetch('/api/config', { headers: { 'Authorization': `Bearer ${OSA.getToken()}` } }).then(r => r.json())
        ]);

        OSA.voiceModels.whisper = modelsRes.whisper || [];
        OSA.voiceModels.piper = modelsRes.piper || [];
        OSA.voiceModels.installed.whisper = installedRes.whisper || [];
        OSA.voiceModels.installed.piper = installedRes.piper || [];

        if (configRes.voice) {
            OSA.voiceModels.selectedWhisper = configRes.voice.whisper_model || null;
            OSA.voiceModels.selectedPiper = configRes.voice.piper_voice || null;
        }
    } catch (error) {
        console.error('Failed to load voice models:', error);
    }
};

OSA.renderVoiceModelBrowser = function() {
    const container = document.getElementById('voice-models-browser');
    if (!container) return;

    let html = `
        <div class="settings-section">
            <div class="settings-section-title">Whisper Models (STT)</div>
            <div class="model-grid" id="whisper-model-grid">
                ${OSA.renderWhisperModels()}
            </div>
            <div class="upload-zone" id="whisper-upload-zone">
                <input type="file" id="whisper-upload-input" accept=".bin" onchange="OSA.handleWhisperUpload(this.files[0])" />
                <label for="whisper-upload-input">
                    <span class="upload-icon">+</span>
                    <span>Drop custom .bin Whisper model or click to upload</span>
                </label>
            </div>
        </div>

        <div class="settings-section">
            <div class="settings-section-title">Piper Voices (TTS)</div>
            <div class="settings-field">
                <label for="piper-language-select">Language</label>
                <select id="piper-language-select" onchange="OSA.onPiperLanguageChange(this.value)">
                    <option value="en" ${OSA.voiceModels.piperLanguage === 'en' ? 'selected' : ''}>English</option>
                    <option value="de" ${OSA.voiceModels.piperLanguage === 'de' ? 'selected' : ''}>German</option>
                    <option value="fr" ${OSA.voiceModels.piperLanguage === 'fr' ? 'selected' : ''}>French</option>
                    <option value="es" ${OSA.voiceModels.piperLanguage === 'es' ? 'selected' : ''}>Spanish</option>
                </select>
            </div>
            <div class="model-grid" id="piper-voice-grid">
                ${OSA.renderPiperVoices()}
            </div>
            <div class="upload-zone" id="piper-upload-zone">
                <input type="file" id="piper-upload-input" accept=".onnx" onchange="OSA.handlePiperUpload(this.files[0])" />
                <label for="piper-upload-input">
                    <span class="upload-icon">+</span>
                    <span>Drop custom .onnx Piper voice or click to upload</span>
                </label>
            </div>
        </div>

        <div class="settings-section">
            <div class="settings-section-title">Installed Models</div>
            <div class="installed-models-list" id="installed-models-list">
                ${OSA.renderInstalledModels()}
            </div>
        </div>
    `;

    container.innerHTML = html;
    OSA.startProgressListener();
};

OSA.renderWhisperModels = function() {
    const models = OSA.voiceModels.whisper;
    if (!models.length) return '<div class="model-empty">No Whisper models available</div>';

    return models.map(m => OSA.renderModelCard(m, 'whisper')).join('');
};

OSA.renderPiperVoices = function() {
    const lang = OSA.voiceModels.piperLanguage;
    const voices = OSA.voiceModels.piper.filter(v => v.lang === lang);
    if (!voices.length) return '<div class="model-empty">No Piper voices available for this language</div>';

    return voices.map(v => OSA.renderModelCard(v, 'piper')).join('');
};

OSA.renderModelCard = function(model, type) {
    const isInstalled = model.installed;
    const isSelected = type === 'whisper' 
        ? OSA.voiceModels.selectedWhisper === model.id
        : OSA.voiceModels.selectedPiper === model.id;
    const progress = OSA.voiceModels.progress[model.id];
    const isDownloading = progress && progress.stage !== 'complete';

    const sizeLabel = model.size_mb > 0 ? `${model.size_mb} MB` : '';
    const qualityLabel = model.quality ? `${model.quality}` : '';
    const metaLabel = [sizeLabel, qualityLabel].filter(Boolean).join(' · ');

    return `
        <div class="model-card ${isSelected ? 'selected' : ''}" data-model-id="${model.id}" data-model-type="${type}">
            <div class="model-card-header">
                <label class="model-radio">
                    <input type="radio" name="model-${type}" value="${model.id}" 
                        ${isSelected ? 'checked' : ''} 
                        onchange="OSA.selectVoiceModel('${type}', '${model.id}')" 
                        ${isDownloading ? 'disabled' : ''} />
                    <span class="model-name">${OSA.escapeHtml(model.name)}</span>
                </label>
            </div>
            ${metaLabel ? `<div class="model-meta">${metaLabel}</div>` : ''}
            ${isDownloading ? `
                <div class="model-progress">
                    <div class="progress-bar">
                        <div class="progress-fill" style="width: ${(progress.progress * 100).toFixed(0)}%"></div>
                    </div>
                    <div class="progress-text">${progress.stage} — ${(progress.progress * 100).toFixed(0)}%</div>
                </div>
            ` : ''}
            <div class="model-actions">
                ${isInstalled ? `
                    <span class="installed-badge">Installed</span>
                ` : `
                    <button class="btn-sm btn-primary" onclick="OSA.downloadModel('${type}', '${model.id}')" ${isDownloading ? 'disabled' : ''}>
                        Download
                    </button>
                `}
            </div>
        </div>
    `;
};

OSA.renderInstalledModels = function() {
    const all = [...OSA.voiceModels.installed.whisper, ...OSA.voiceModels.installed.piper];
    if (!all.length) return '<div class="model-empty">No models installed yet</div>';

    return all.map(m => `
        <div class="installed-model-row" data-model-id="${m.id}" data-model-type="${m.model_type}">
            <div class="installed-model-info">
                <span class="installed-model-type ${m.model_type}">${m.model_type}</span>
                <span class="installed-model-name">${OSA.escapeHtml(m.name)}</span>
                <span class="installed-model-size">${(m.size_bytes / (1024*1024)).toFixed(1)} MB</span>
            </div>
            <button class="btn-sm btn-danger" onclick="OSA.deleteModel('${m.model_type}', '${m.id}')">Delete</button>
        </div>
    `).join('');
};

OSA.selectVoiceModel = async function(type, modelId) {
    try {
        const configRes = await fetch('/api/config', { headers: { 'Authorization': `Bearer ${OSA.getToken()}` } });
        const config = await configRes.json();
        
        if (!config.voice) config.voice = {};
        if (type === 'whisper') {
            config.voice.whisper_model = modelId;
            OSA.voiceModels.selectedWhisper = modelId;
        } else {
            config.voice.piper_voice = modelId;
            OSA.voiceModels.selectedPiper = modelId;
        }

        const saveRes = await fetch('/api/config', {
            method: 'PUT',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify(config)
        });

        if (saveRes.ok) {
            OSA.renderVoiceModelBrowser();
        }
    } catch (error) {
        console.error('Failed to save model selection:', error);
    }
};

OSA.downloadModel = async function(type, modelId) {
    OSA.voiceModels.progress[modelId] = { stage: 'starting', progress: 0 };

    try {
        const res = await fetch('/api/voice/download', {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}`, 'Content-Type': 'application/json' },
            body: JSON.stringify({ model_type: type, model_id: modelId })
        });

        if (!res.ok) {
            const data = await res.json();
            throw new Error(data.error || 'Download failed');
        }
    } catch (error) {
        console.error('Download failed:', error);
        delete OSA.voiceModels.progress[modelId];
        OSA.renderVoiceModelBrowser();
    }
};

OSA.deleteModel = async function(type, modelId) {
    if (!confirm(`Delete ${type} model '${modelId}'?`)) return;

    try {
        const res = await fetch(`/api/voice/model/${type}/${encodeURIComponent(modelId)}`, {
            method: 'DELETE',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` }
        });

        if (!res.ok) {
            const data = await res.json();
            throw new Error(data.error || 'Delete failed');
        }

        await OSA.loadVoiceModels();
        OSA.renderVoiceModelBrowser();
    } catch (error) {
        console.error('Delete failed:', error);
        alert(`Failed to delete model: ${error.message}`);
    }
};

OSA.handleWhisperUpload = async function(file) {
    if (!file) return;
    await OSA.uploadModel(file, 'whisper');
};

OSA.handlePiperUpload = async function(file) {
    if (!file) return;
    await OSA.uploadModel(file, 'piper');
};

OSA.uploadModel = async function(file, type) {
    const expectedExt = type === 'whisper' ? '.bin' : '.onnx';
    if (!file.name.toLowerCase().endsWith(expectedExt)) {
        alert(`Please upload a ${expectedExt} file for ${type} models.`);
        return;
    }

    try {
        const formData = new FormData();
        formData.append('file', file);

        const res = await fetch(`/api/voice/upload?type=${type}`, {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${OSA.getToken()}` },
            body: formData
        });

        const data = await res.json();
        if (!res.ok) throw new Error(data.error || 'Upload failed');

        alert(`Uploaded: ${data.message}`);
        await OSA.loadVoiceModels();
        OSA.renderVoiceModelBrowser();
    } catch (error) {
        console.error('Upload failed:', error);
        alert(`Upload failed: ${error.message}`);
    }
};

OSA.onPiperLanguageChange = function(lang) {
    OSA.voiceModels.piperLanguage = lang;
    OSA.renderVoiceModelBrowser();
};

OSA.startProgressListener = function() {
    if (OSA.voiceModels.eventSource) {
        OSA.voiceModels.eventSource.close();
    }

    OSA.voiceModels.eventSource = new EventSource('/api/voice/progress');

    OSA.voiceModels.eventSource.onmessage = function(event) {
        try {
            const progress = JSON.parse(event.data);
            OSA.voiceModels.progress[progress.model_id] = progress;

            const card = document.querySelector(`.model-card[data-model-id="${progress.model_id}"]`);
            if (card) {
                const progressDiv = card.querySelector('.model-progress');
                if (progressDiv) {
                    progressDiv.innerHTML = `
                        <div class="progress-bar">
                            <div class="progress-fill" style="width: ${(progress.progress * 100).toFixed(0)}%"></div>
                        </div>
                        <div class="progress-text">${progress.stage} — ${(progress.progress * 100).toFixed(0)}%</div>
                    `;
                }
            }

            if (progress.stage === 'complete') {
                setTimeout(async () => {
                    delete OSA.voiceModels.progress[progress.model_id];
                    await OSA.loadVoiceModels();
                    OSA.renderVoiceModelBrowser();
                }, 1000);
            }
        } catch (e) {
            console.error('Failed to parse progress:', e);
        }
    };

    OSA.voiceModels.eventSource.onerror = function() {
        console.error('Progress SSE error');
    };
};

OSA.stopProgressListener = function() {
    if (OSA.voiceModels.eventSource) {
        OSA.voiceModels.eventSource.close();
        OSA.voiceModels.eventSource = null;
    }
};

OSA.escapeHtml = function(text) {
    if (!text) return '';
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
};
