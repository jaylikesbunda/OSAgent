window.OSA = window.OSA || {};

OSA.voiceModels = {
    whisper: [],
    piper: [],
    installed: { whisper: [], piper: [] },
    progress: {},
    installing: { whisper: false, piper: false },
    status: null,
    selectedWhisper: null,
    selectedPiper: null,
    piperLanguage: 'en',
    eventSource: null,
    renderTimer: null,
    lastRenderAt: 0
};

OSA.isVoicePaneActive = function() {
    return !!document.getElementById('pane-voice')?.classList.contains('active');
};

OSA.formatVoiceBytes = function(bytes) {
    if (!bytes || bytes <= 0) return '0 MB';
    return `${(bytes / (1024 * 1024)).toFixed(bytes >= 1024 * 1024 * 1024 ? 2 : 1)} MB`;
};

OSA.queueVoiceModelsRender = function() {
    if (!OSA.isVoicePaneActive()) {
        return;
    }

    const now = Date.now();
    const minInterval = 120;
    const elapsed = now - (OSA.voiceModels.lastRenderAt || 0);

    if (elapsed >= minInterval && !OSA.voiceModels.renderTimer) {
        OSA.voiceModels.lastRenderAt = now;
        OSA.renderVoiceModelBrowser();
        return;
    }

    if (OSA.voiceModels.renderTimer) {
        return;
    }

    OSA.voiceModels.renderTimer = window.setTimeout(() => {
        OSA.voiceModels.renderTimer = null;
        OSA.voiceModels.lastRenderAt = Date.now();
        if (OSA.isVoicePaneActive()) {
            OSA.renderVoiceModelBrowser();
        }
    }, Math.max(0, minInterval - elapsed));
};

OSA.findVoiceModelById = function(type, modelId) {
    const list = type === 'whisper' ? OSA.voiceModels.whisper : OSA.voiceModels.piper;
    return list.find(model => model.id === modelId) || null;
};

OSA.getInstalledIds = function(type) {
    return new Set((OSA.voiceModels.installed[type] || []).map(model => model.id));
};

OSA.isVoiceModelInstalled = function(type, modelId) {
    if (!modelId) return false;
    return OSA.getInstalledIds(type).has(modelId);
};

OSA.getVoiceProgressForType = function(type) {
    const entries = Object.values(OSA.voiceModels.progress || {});
    return entries.find(progress => progress.model_type === type && progress.stage !== 'complete') || null;
};

OSA.renderDownloadProgress = function(progress) {
    if (!progress) return '';
    const percent = (progress.progress * 100).toFixed(0);
    const hasTotal = !!progress.total_bytes;
    const details = hasTotal
        ? `${OSA.formatVoiceBytes(progress.bytes_downloaded)} / ${OSA.formatVoiceBytes(progress.total_bytes)}`
        : OSA.formatVoiceBytes(progress.bytes_downloaded);

    return `
        <div class="model-progress">
            <div class="progress-bar">
                <div class="progress-fill" style="width: ${percent}%"></div>
            </div>
            <div class="progress-text">${OSA.escapeHtml(progress.stage)} - ${percent}%${details ? ` - ${details}` : ''}</div>
        </div>
    `;
};

OSA.getDefaultPiperVoiceForLanguage = function(lang) {
    const voices = OSA.voiceModels.piper.filter(voice => voice.lang === lang);
    return voices[0]?.id || null;
};

OSA.getWhisperVramRequirement = function(model) {
    if (!model || model.model_type !== 'whisper') return null;

    switch ((model.id || '').toLowerCase()) {
        case 'tiny':
            return 'Approx VRAM: 1 GB';
        case 'base':
            return 'Approx VRAM: 1 GB';
        case 'small':
            return 'Approx VRAM: 2 GB';
        case 'medium':
            return 'Approx VRAM: 5 GB';
        default:
            if ((model.size_mb || 0) >= 1400) return 'Approx VRAM: 5+ GB';
            if ((model.size_mb || 0) >= 450) return 'Approx VRAM: 2+ GB';
            if ((model.size_mb || 0) > 0) return 'Approx VRAM: 1+ GB';
            return 'VRAM varies by model';
    }
};

OSA.fetchVoiceJson = async function(url) {
    const response = await OSA.fetchWithAuth(url);
    const data = await response.json().catch(() => ({}));
    if (!response.ok) {
        throw new Error(data.error || `HTTP ${response.status}`);
    }
    return data;
};

OSA.loadVoiceModels = async function() {
    const [modelsRes, installedRes, configRes, statusRes] = await Promise.all([
        OSA.fetchVoiceJson('/api/voice/models'),
        OSA.fetchVoiceJson('/api/voice/installed'),
        OSA.fetchVoiceJson('/api/config'),
        OSA.fetchVoiceJson('/api/voice/status')
    ]);

    const voiceConfig = OSA.normalizeVoiceConfig(configRes.voice || {});
    OSA.voiceModels.whisper = modelsRes.whisper || [];
    OSA.voiceModels.piper = modelsRes.piper || [];
    OSA.voiceModels.installed.whisper = installedRes.whisper || [];
    OSA.voiceModels.installed.piper = installedRes.piper || [];
    OSA.voiceModels.status = statusRes || null;

    const installedWhisperIds = new Set(OSA.voiceModels.installed.whisper.map(model => model.id));
    const availableWhisperIds = new Set(OSA.voiceModels.whisper.map(model => model.id));
    const configuredWhisper = voiceConfig?.whisper_model || null;
    OSA.voiceModels.selectedWhisper = configuredWhisper && (installedWhisperIds.has(configuredWhisper) || availableWhisperIds.has(configuredWhisper))
        ? configuredWhisper
        : (OSA.voiceModels.installed.whisper[0]?.id || OSA.voiceModels.whisper[0]?.id || 'base');

    const installedPiperIds = new Set(OSA.voiceModels.installed.piper.map(model => model.id));
    const availablePiperIds = new Set(OSA.voiceModels.piper.map(model => model.id));
    const configuredPiper = voiceConfig?.piper_voice || null;
    const configuredPiperModel = configuredPiper ? OSA.findVoiceModelById('piper', configuredPiper) : null;
    OSA.voiceModels.piperLanguage = configuredPiperModel?.lang || voiceConfig?.language || 'en';
    OSA.voiceModels.selectedPiper = configuredPiper && (installedPiperIds.has(configuredPiper) || availablePiperIds.has(configuredPiper))
        ? configuredPiper
        : (OSA.voiceModels.installed.piper[0]?.id || OSA.getDefaultPiperVoiceForLanguage(OSA.voiceModels.piperLanguage));
};

OSA.renderRuntimeCard = function(type) {
    const status = OSA.voiceModels.status || {};
    const progress = OSA.getVoiceProgressForType(type);
    const isInstalling = !!OSA.voiceModels.installing[type];
    const isWhisper = type === 'whisper';
    const selectedId = isWhisper ? (OSA.voiceModels.selectedWhisper || 'base') : OSA.voiceModels.selectedPiper;
    const selectedModel = OSA.findVoiceModelById(type, selectedId);
    const runtimeInstalled = isWhisper ? !!status.whisper_installed : !!status.piper_installed;
    const selectedInstalled = OSA.isVoiceModelInstalled(type, selectedId);
    const runtimeReady = runtimeInstalled;
    const fullyReady = runtimeInstalled && selectedInstalled;
    const currentDownloaded = isWhisper ? (status.whisper_model || 'none') : (status.piper_voice || 'none');
    const selectedLabel = selectedModel?.name || selectedId || 'Not selected';
    const badgeLabel = fullyReady ? 'Ready' : (runtimeReady ? 'Runtime ready' : 'Runtime missing');
    const actionLabel = !runtimeInstalled
        ? (isWhisper ? 'Install runtime + selected model' : 'Install runtime + selected voice')
        : (selectedInstalled
            ? (isWhisper ? 'Repair runtime' : 'Repair runtime')
            : (isWhisper ? 'Download selected model' : 'Download selected voice'));
    const hintText = !runtimeInstalled
        ? (isWhisper ? 'Install this before using the mic with Local Whisper.' : 'Install this before enabling Local Piper speech.')
        : (selectedInstalled
            ? 'Everything needed for this selection is installed.'
            : (isWhisper ? 'The runtime is installed, but the selected Whisper model still needs to be downloaded.' : 'The runtime is installed, but the selected Piper voice still needs to be downloaded.'));

    return `
        <div class="voice-runtime-card ${runtimeReady ? 'ready' : ''}" data-runtime-type="${type}">
            <div class="voice-runtime-header">
                <div>
                    <div class="voice-runtime-title">${isWhisper ? 'Local Whisper' : 'Local Piper'}</div>
                    <div class="voice-runtime-subtitle">${isWhisper ? 'Mic transcription into chat' : 'Local text-to-speech playback'}</div>
                </div>
                <span class="voice-runtime-badge ${runtimeReady ? 'ready' : 'missing'}">${badgeLabel}</span>
            </div>
            <div class="voice-runtime-meta">
                <span>${runtimeInstalled ? 'Runtime installed' : 'Runtime missing'}</span>
                <span>Selected: ${OSA.escapeHtml(selectedLabel)}</span>
                <span>${selectedInstalled ? 'Selected file downloaded' : 'Selected file missing'}</span>
                <span>Current downloaded: ${OSA.escapeHtml(currentDownloaded)}</span>
            </div>
            ${OSA.renderDownloadProgress(progress)}
            <div class="voice-runtime-actions">
                <button class="voice-action-btn voice-action-btn-primary voice-runtime-btn" onclick="OSA.installVoiceRuntime('${type}')" ${isInstalling ? 'disabled' : ''}>
                    ${isInstalling ? 'Installing...' : actionLabel}
                </button>
                <span class="voice-runtime-hint">${hintText}</span>
            </div>
        </div>
    `;
};

OSA.renderVoiceModelBrowser = function() {
    const container = document.getElementById('voice-models-browser');
    if (!container) return;

    const selectedWhisperInstalled = OSA.isVoiceModelInstalled('whisper', OSA.voiceModels.selectedWhisper);
    const selectedPiperInstalled = OSA.isVoiceModelInstalled('piper', OSA.voiceModels.selectedPiper);

    container.innerHTML = `
        <div class="settings-section">
            <div class="settings-section-title">Local Voice Setup</div>
            <div class="voice-runtime-grid">
                ${OSA.renderRuntimeCard('whisper')}
                ${OSA.renderRuntimeCard('piper')}
            </div>
            <div class="voice-runtime-note">
                Install the runtime first, then download any extra models or voices you want to switch between later.
            </div>
        </div>

        <div class="settings-section">
            <div class="settings-section-title">Whisper Models (STT)</div>
            <div class="voice-selection-summary ${selectedWhisperInstalled ? 'ready' : 'missing'}">
                Selected model: <strong>${OSA.escapeHtml(OSA.voiceModels.selectedWhisper || 'base')}</strong>
                ${selectedWhisperInstalled ? 'is ready to use.' : 'still needs to be downloaded or installed above.'}
            </div>
            <div class="model-grid" id="whisper-model-grid">
                ${OSA.renderWhisperModels()}
            </div>
            <div class="upload-zone" id="whisper-upload-zone">
                <input type="file" id="whisper-upload-input" accept=".bin" onchange="OSA.handleWhisperUpload(this.files[0])" />
                <label for="whisper-upload-input">
                    <span class="upload-icon">+</span>
                    <span>Upload a custom Whisper <code>.bin</code> model</span>
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
            <div class="voice-selection-summary ${selectedPiperInstalled ? 'ready' : 'missing'}">
                Selected voice: <strong>${OSA.escapeHtml(OSA.voiceModels.selectedPiper || 'none')}</strong>
                ${selectedPiperInstalled ? 'is ready to use.' : 'still needs to be downloaded or installed above.'}
            </div>
            <div class="model-grid" id="piper-voice-grid">
                ${OSA.renderPiperVoices()}
            </div>
            <div class="upload-zone" id="piper-upload-zone">
                <input type="file" id="piper-upload-input" accept=".onnx" onchange="OSA.handlePiperUpload(this.files[0])" />
                <label for="piper-upload-input">
                    <span class="upload-icon">+</span>
                    <span>Upload a custom Piper <code>.onnx</code> voice</span>
                </label>
            </div>
        </div>

        <div class="settings-section">
            <div class="settings-section-title">Downloaded Files</div>
            <div class="installed-models-list" id="installed-models-list">
                ${OSA.renderInstalledModels()}
            </div>
        </div>
    `;

    OSA.startProgressListener();
};

OSA.renderWhisperModels = function() {
    const models = OSA.voiceModels.whisper;
    if (!models.length) return '<div class="model-empty">No Whisper models available</div>';
    return models.map(model => OSA.renderModelCard(model, 'whisper')).join('');
};

OSA.renderPiperVoices = function() {
    const lang = OSA.voiceModels.piperLanguage;
    const voices = OSA.voiceModels.piper.filter(voice => voice.lang === lang);
    if (!voices.length) return '<div class="model-empty">No Piper voices available for this language</div>';
    return voices.map(voice => OSA.renderModelCard(voice, 'piper')).join('');
};

OSA.renderModelCard = function(model, type) {
    const isInstalled = OSA.isVoiceModelInstalled(type, model.id);
    const isSelected = type === 'whisper'
        ? OSA.voiceModels.selectedWhisper === model.id
        : OSA.voiceModels.selectedPiper === model.id;
    const progress = OSA.voiceModels.progress[model.id];
    const isDownloading = !!progress && progress.stage !== 'complete';
    const sizeLabel = model.size_mb > 0 ? `${model.size_mb} MB` : '';
    const qualityLabel = model.quality ? `${model.quality}` : '';
    const metaLabel = [sizeLabel, qualityLabel].filter(Boolean).join(' - ');
    const whisperVram = OSA.getWhisperVramRequirement(model);

    return `
        <div class="model-card ${isSelected ? 'selected' : ''}" data-model-id="${model.id}" data-model-type="${type}">
            <div class="model-card-header">
                <label class="model-radio">
                    <input
                        type="radio"
                        name="model-${type}"
                        value="${model.id}"
                        ${isSelected ? 'checked' : ''}
                        onchange="OSA.selectVoiceModel('${type}', '${model.id}')"
                        ${(isDownloading || !isInstalled) ? 'disabled' : ''}
                    />
                    <span class="model-name">${OSA.escapeHtml(model.name)}</span>
                </label>
            </div>
            ${metaLabel ? `<div class="model-meta">${OSA.escapeHtml(metaLabel)}</div>` : ''}
            ${whisperVram ? `<div class="model-requirements">${OSA.escapeHtml(whisperVram)}</div>` : ''}
            ${OSA.renderDownloadProgress(progress)}
            <div class="model-actions">
                ${isInstalled
                    ? '<span class="installed-badge">Downloaded</span>'
                    : `<button class="voice-action-btn voice-action-btn-primary" onclick="OSA.downloadModel('${type}', '${model.id}')" ${isDownloading ? 'disabled' : ''}>Download</button>`}
                ${!isInstalled ? '<span class="model-help">Download before selecting</span>' : ''}
            </div>
        </div>
    `;
};

OSA.renderInstalledModels = function() {
    const all = [...OSA.voiceModels.installed.whisper, ...OSA.voiceModels.installed.piper];
    if (!all.length) return '<div class="model-empty">No models or voices downloaded yet</div>';

    return all.map(model => `
        <div class="installed-model-row" data-model-id="${model.id}" data-model-type="${model.model_type}">
            <div class="installed-model-info">
                <span class="installed-model-type ${model.model_type}">${model.model_type}</span>
                <span class="installed-model-name">${OSA.escapeHtml(model.name)}</span>
                <span class="installed-model-size">${(model.size_bytes / (1024 * 1024)).toFixed(1)} MB</span>
            </div>
            <button class="voice-action-btn voice-action-btn-danger" onclick="OSA.deleteModel('${model.model_type}', '${model.id}')">Delete</button>
        </div>
    `).join('');
};

OSA.selectVoiceModel = async function(type, modelId) {
    if (!OSA.isVoiceModelInstalled(type, modelId)) {
        return;
    }

    try {
        const configRes = await OSA.fetchWithAuth('/api/config');
        const config = await configRes.json();
        config.voice = OSA.normalizeVoiceConfig(config.voice || {});

        if (type === 'whisper') {
            config.voice.whisper_model = modelId;
            OSA.voiceModels.selectedWhisper = modelId;
        } else {
            config.voice.piper_voice = modelId;
            OSA.voiceModels.selectedPiper = modelId;
        }

        const saveRes = await OSA.fetchWithAuth('/api/config', {
            method: 'PUT',
            body: JSON.stringify(config)
        });

        if (!saveRes.ok) {
            const data = await saveRes.json().catch(() => ({}));
            throw new Error(data.error || `HTTP ${saveRes.status}`);
        }

        OSA.setCachedConfig(config);
        OSA.setVoiceConfig(config.voice);
        OSA.updateVoiceButtons();
        OSA.renderVoiceModelBrowser();
    } catch (error) {
        console.error('Failed to save model selection:', error);
        alert(`Failed to save selection: ${error.message}`);
    }
};

OSA.installVoiceRuntime = async function(type) {
    OSA.voiceModels.installing[type] = true;
    OSA.queueVoiceModelsRender();

    try {
        let payload;
        if (type === 'whisper') {
            payload = {
                install_whisper: true,
                whisper_model: OSA.voiceModels.selectedWhisper || 'base'
            };
        } else {
            const selectedVoice = OSA.voiceModels.selectedPiper || OSA.getDefaultPiperVoiceForLanguage(OSA.voiceModels.piperLanguage);
            const selectedModel = OSA.findVoiceModelById('piper', selectedVoice);
            payload = {
                install_piper: true,
                language: selectedModel?.lang || OSA.voiceModels.piperLanguage || 'en',
                piper_voice: selectedVoice
            };
        }

        const response = await OSA.fetchWithAuth('/api/voice/install', {
            method: 'POST',
            body: JSON.stringify(payload)
        });
        const data = await response.json().catch(() => ({}));
        if (!response.ok) {
            throw new Error(data.error || `HTTP ${response.status}`);
        }

        await OSA.loadVoiceModels();
        OSA.queueVoiceModelsRender();
    } catch (error) {
        console.error('Voice install failed:', error);
        alert(`Install failed: ${error.message}`);
    } finally {
        OSA.voiceModels.installing[type] = false;
        OSA.queueVoiceModelsRender();
    }
};

OSA.downloadModel = async function(type, modelId) {
    OSA.voiceModels.progress[modelId] = { model_id: modelId, model_type: type, stage: 'starting', progress: 0 };
    OSA.queueVoiceModelsRender();

    try {
        const response = await OSA.fetchWithAuth('/api/voice/download', {
            method: 'POST',
            body: JSON.stringify({ model_type: type, model_id: modelId })
        });

        if (!response.ok) {
            const data = await response.json().catch(() => ({}));
            throw new Error(data.error || 'Download failed');
        }
    } catch (error) {
        console.error('Download failed:', error);
        delete OSA.voiceModels.progress[modelId];
        OSA.queueVoiceModelsRender();
        alert(`Download failed: ${error.message}`);
    }
};

OSA.deleteModel = async function(type, modelId) {
    if (!confirm(`Delete ${type} '${modelId}'?`)) return;

    try {
        const response = await OSA.fetchWithAuth(`/api/voice/model/${type}/${encodeURIComponent(modelId)}`, {
            method: 'DELETE'
        });
        const data = await response.json().catch(() => ({}));

        if (!response.ok) {
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
        alert(`Please upload a ${expectedExt} file for ${type}.`);
        return;
    }

    try {
        const response = await OSA.fetchWithAuth(`/api/voice/upload?type=${type}`, {
            method: 'POST',
            headers: {
                'Authorization': `Bearer ${OSA.getToken()}`,
                'Content-Type': 'application/octet-stream'
            },
            body: file
        });

        const data = await response.json().catch(() => ({}));
        if (!response.ok) {
            throw new Error(data.error || 'Upload failed');
        }

        alert(data.message || 'Upload complete');
        await OSA.loadVoiceModels();
        OSA.renderVoiceModelBrowser();
    } catch (error) {
        console.error('Upload failed:', error);
        alert(`Upload failed: ${error.message}`);
    }
};

OSA.onPiperLanguageChange = function(lang) {
    OSA.voiceModels.piperLanguage = lang;
    if (!OSA.findVoiceModelById('piper', OSA.voiceModels.selectedPiper)?.lang || !OSA.voiceModels.piper.some(voice => voice.id === OSA.voiceModels.selectedPiper && voice.lang === lang)) {
        OSA.voiceModels.selectedPiper = OSA.getDefaultPiperVoiceForLanguage(lang);
    }
    OSA.renderVoiceModelBrowser();
};

OSA.startProgressListener = function() {
    if (OSA.voiceModels.eventSource) {
        return;
    }

    OSA.voiceModels.eventSource = new EventSource('/api/voice/progress');

    OSA.voiceModels.eventSource.addEventListener('progress', function(event) {
        try {
            const progress = JSON.parse(event.data);
            OSA.voiceModels.progress[progress.model_id] = progress;
            OSA.queueVoiceModelsRender();

            if (progress.stage === 'complete') {
                setTimeout(async () => {
                    delete OSA.voiceModels.progress[progress.model_id];
                    await OSA.loadVoiceModels();
                    OSA.queueVoiceModelsRender();
                }, 800);
            }
        } catch (error) {
            console.error('Failed to parse voice progress:', error);
        }
    });

    OSA.voiceModels.eventSource.onerror = function() {
        console.error('Voice progress SSE error');
    };
};

OSA.stopProgressListener = function() {
    if (OSA.voiceModels.eventSource) {
        OSA.voiceModels.eventSource.close();
        OSA.voiceModels.eventSource = null;
    }
};
