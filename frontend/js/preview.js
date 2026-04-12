window.OSA = window.OSA || {};

OSA._previewState = {
    open: false,
    width: 420,
    path: '',
    mode: 'file',
};

OSA.initSplitPane = function() {
    const handle = document.getElementById('preview-resize-handle');
    const appView = document.getElementById('app-view');
    if (!handle || !appView) return;

    let dragging = false;

    const onMove = function(event) {
        if (!dragging || !OSA._previewState.open) return;
        const x = event.clientX || 0;
        const nextWidth = Math.min(900, Math.max(280, window.innerWidth - x));
        OSA._previewState.width = nextWidth;
        appView.style.setProperty('--preview-width', `${nextWidth}px`);
    };

    const onUp = function() {
        dragging = false;
        document.body.classList.remove('resizing-preview');
    };

    handle.addEventListener('mousedown', function(event) {
        if (!OSA._previewState.open) return;
        event.preventDefault();
        dragging = true;
        document.body.classList.add('resizing-preview');
    });

    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
};

OSA.openFilePreview = function(path, content, options) {
    const panel = document.getElementById('file-preview-panel');
    const handle = document.getElementById('preview-resize-handle');
    const appView = document.getElementById('app-view');
    const pathEl = document.getElementById('file-preview-path');
    const body = document.getElementById('file-preview-body');
    if (!panel || !handle || !appView || !pathEl || !body) return;

    const previewOptions = options && typeof options === 'object' ? options : {};
    const previewMode = previewOptions.mode === 'diff' ? 'diff' : 'file';

    OSA._previewState.open = true;
    OSA._previewState.path = path || '';
    OSA._previewState.mode = previewMode;
    appView.classList.add('with-preview');
    appView.style.setProperty('--preview-width', `${OSA._previewState.width}px`);
    panel.classList.remove('hidden');
    handle.classList.remove('hidden');
    pathEl.textContent = path || 'File preview';
    const text = content || '';

    if (previewMode === 'diff' && typeof OSA.renderDiffView === 'function') {
        body.innerHTML = '';
        body.appendChild(OSA.renderDiffView(previewOptions.oldContent || '', previewOptions.newContent || text));
        return;
    }

    const language = OSA.guessLanguageFromPath(path || '');
    body.innerHTML = OSA.renderFilePreviewBody(path || '', text, language || '');
};

OSA.closeFilePreview = function() {
    const panel = document.getElementById('file-preview-panel');
    const handle = document.getElementById('preview-resize-handle');
    const appView = document.getElementById('app-view');
    if (!panel || !handle || !appView) return;

    OSA._previewState.open = false;
    OSA._previewState.mode = 'file';
    appView.classList.remove('with-preview');
    panel.classList.add('hidden');
    handle.classList.add('hidden');
};

OSA.toggleFilePreview = function() {
    if (OSA._previewState.open) {
        OSA.closeFilePreview();
        return;
    }
    OSA.openFilePreview(OSA._previewState.path || 'File preview', 'Preview is empty. Use read_file or edit tools to populate this panel.');
};

OSA.showFilePreviewFromDiff = function(fileDiff) {
    if (!fileDiff || !fileDiff.path) return;
    const content = typeof fileDiff.new_content === 'string'
        ? fileDiff.new_content
        : (typeof fileDiff.old_content === 'string' ? fileDiff.old_content : '');
    OSA.openFilePreview(fileDiff.path, content, {
        mode: 'diff',
        oldContent: typeof fileDiff.old_content === 'string' ? fileDiff.old_content : '',
        newContent: typeof fileDiff.new_content === 'string' ? fileDiff.new_content : content,
    });
};

OSA.guessLanguageFromPath = function(path) {
    const lower = (path || '').toLowerCase();
    if (lower.endsWith('.md') || lower.endsWith('.markdown')) return 'markdown';
    if (lower.endsWith('.json')) return 'json';
    if (lower.endsWith('.toml')) return 'toml';
    if (lower.endsWith('.yaml') || lower.endsWith('.yml')) return 'yaml';
    if (lower.endsWith('.html')) return 'html';
    if (lower.endsWith('.css')) return 'css';
    if (lower.endsWith('.rs')) return 'rust';
    if (lower.endsWith('.js') || lower.endsWith('.mjs') || lower.endsWith('.cjs')) return 'javascript';
    if (lower.endsWith('.ts') || lower.endsWith('.tsx')) return 'javascript';
    if (lower.endsWith('.py')) return 'python';
    if (lower.endsWith('.java')) return 'java';
    if (lower.endsWith('.cpp') || lower.endsWith('.cc') || lower.endsWith('.cxx')) return 'cpp';
    if (lower.endsWith('.c') || lower.endsWith('.h')) return 'c';
    return '';
};

OSA.renderFilePreviewBody = function(path, text, language) {
    const lower = (path || '').toLowerCase();

    if (language === 'markdown' && typeof OSA.formatMessage === 'function') {
        return '<div class="file-preview-markdown">' + OSA.formatMessage(text) + '</div>';
    }

    const displayLanguage = language || 'text';
    const highlighted = language && language !== 'markdown' && typeof OSA.highlightCode === 'function'
        ? OSA.highlightCode(text, language)
        : OSA.escapeHtml(text);

    return ''
        + '<div class="code-block file-preview-code">'
        + '  <div class="code-header">'
        + '    <span class="code-lang">' + OSA.escapeHtml(displayLanguage) + '</span>'
        + '    <button class="code-copy" onclick="OSA.copyPreviewCode(this)">Copy</button>'
        + '  </div>'
        + '  <pre><code class="language-' + OSA.escapeHtml(displayLanguage) + '">' + highlighted + '</code></pre>'
        + '</div>';
};

OSA.copyPreviewCode = function(button) {
    const code = button.closest('.file-preview-code')?.querySelector('code')?.textContent || '';
    navigator.clipboard.writeText(code).then(function() {
        button.textContent = 'Copied!';
        setTimeout(function() {
            button.textContent = 'Copy';
        }, 2000);
    });
};

window.addEventListener('DOMContentLoaded', function() {
    OSA.initSplitPane();
});
