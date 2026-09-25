const assert = require('node:assert/strict');
const test = require('node:test');

let Window;

test.before(async () => {
    ({ Window } = await import('happy-dom'));
    const window = new Window({ url: 'http://localhost/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.localStorage = window.localStorage;
    global.sessionStorage = window.sessionStorage;
    global.requestAnimationFrame = () => 1;
    global.cancelAnimationFrame = () => {};
    global.OSA = window.OSA = {};

    require('../js/utils.js');
    require('../js/api.js');
    require('../js/settings.js');
});

function mountUpdatePane() {
    document.body.innerHTML = `
        <div id="settings-modal" class="modal hidden">
            <div class="settings-pane active" id="pane-updates">
                <span id="update-current-version">-</span>
                <div id="update-status-display"><span class="update-status-text"></span></div>
                <div id="update-version-row" class="hidden"><span id="update-latest-version">-</span></div>
                <div class="update-actions">
                    <button id="btn-check-update" onclick="OSA.handleUpdateAction()">Check for Updates</button>
                    <a id="btn-view-release" class="hidden"></a>
                </div>
                <div id="update-progress-container" class="hidden">
                    <div class="progress-bar"><div id="update-progress-fill"></div></div>
                    <span id="update-progress-text">0%</span>
                </div>
                <div id="update-release-notes" class="hidden"><div id="release-notes-content"></div></div>
                <select id="update-channel-select"><option value="stable">Stable</option></select>
            </div>
        </div>
        <div id="settings-error" class="hidden"></div>
    `;
    OSA._updateState = {
        status: 'idle',
        phase: 'idle',
        retryAction: null,
        releaseUrl: '',
        releaseNotes: ''
    };
    OSA.pendingUpdateTag = null;
    OSA.pendingUpdateVersion = null;
    OSA.currentVersion = null;
    OSA._updatePollGeneration = 0;
    OSA._updatePollTimer = null;
    OSA._updateStartupToastPending = false;
    OSA.UPDATE_POLL_INTERVAL = 0;
    OSA.stopUpdatePolling();
}

test('update errors are rejected when the server returns HTTP 200 with an error body', async () => {
    mountUpdatePane();
    OSA.fetchWithAuth = async () => ({
        ok: true,
        status: 200,
        json: async () => ({ error: 'staging failed' })
    });

    await assert.rejects(
        OSA.fetchUpdateJson('/api/update/download', undefined, 'Download failed'),
        /staging failed/
    );
});

test('the single update action follows available, downloading, and ready states', () => {
    mountUpdatePane();
    OSA.renderUpdateStatus({
        current_version: '1.2.3',
        update_available: true,
        latest_version: '1.3.0',
        latest_tag: 'v1.3.0',
        release_url: 'https://github.com/example/osagent/releases/tag/v1.3.0',
        release_notes: '<img src=x onerror=alert(1)>',
    });

    const button = document.getElementById('btn-check-update');
    assert.equal(document.querySelectorAll('.update-actions button').length, 1);
    assert.equal(button.textContent, 'Download Update');
    assert.equal(button.disabled, false);
    assert.equal(OSA.pendingUpdateTag, 'v1.3.0');
    assert.equal(document.getElementById('release-notes-content').textContent, '<img src=x onerror=alert(1)>');
    assert.equal(document.getElementById('release-notes-content').querySelector('img'), null);
    assert.equal(document.getElementById('btn-view-release').href, 'https://github.com/example/osagent/releases/tag/v1.3.0');

    OSA.renderUpdateStatus({
        status: 'downloading',
        progress: 37.5,
        bytes_downloaded: 375,
        total_bytes: 1000,
    });
    assert.equal(button.textContent, 'Downloading…');
    assert.equal(button.disabled, true);
    assert.equal(document.getElementById('update-progress-fill').style.width, '37.5%');
    assert.match(document.getElementById('update-progress-text').textContent, /375 \/ 1000 bytes/);

    OSA.renderUpdateStatus({ status: 'ready', tag: 'v1.3.0', version: '1.3.0', progress: 100 });
    assert.equal(button.textContent, 'Install & Restart');
    assert.equal(button.disabled, false);
    assert.equal(OSA.pendingUpdateTag, 'v1.3.0');
});

test('update status polling is recursive, generation guarded, and stops on close', async () => {
    mountUpdatePane();
    let calls = 0;
    OSA.fetchWithAuth = async () => {
        calls += 1;
        return {
            ok: true,
            status: 200,
            json: async () => calls === 1
                ? { status: 'downloading', progress: 10 }
                : { status: 'ready', tag: 'v2.0.0', version: '2.0.0', progress: 100 }
        };
    };

    OSA.startUpdatePolling();
    await new Promise(resolve => setTimeout(resolve, 20));
    assert.equal(calls, 2);
    assert.equal(OSA._updateState.phase, 'ready');
    assert.equal(OSA._updatePollTimer, null);

    calls = 0;
    OSA.startUpdatePolling();
    OSA.closeSettings();
    await new Promise(resolve => setTimeout(resolve, 10));
    assert.equal(calls, 0);
});

test('download and install failures keep a safe, enabled retry action', async () => {
    mountUpdatePane();
    OSA.pendingUpdateTag = 'v2.0.0';
    OSA._updateState.phase = 'available';
    OSA._updateState.status = 'available';
    OSA.fetchWithAuth = async () => ({
        ok: true,
        status: 200,
        json: async () => ({ error: 'network disappeared' })
    });

    assert.equal(await OSA.downloadUpdate(), false);
    assert.equal(OSA._updateState.phase, 'error');
    assert.equal(OSA._updateState.retryAction, 'download');
    assert.equal(document.getElementById('btn-check-update').textContent, 'Retry Download');
    assert.equal(document.getElementById('btn-check-update').disabled, false);

    OSA._updateState.phase = 'ready';
    OSA._updateState.status = 'ready';
    assert.equal(await OSA.installUpdate(), false);
    assert.equal(OSA._updateState.retryAction, 'install');
    assert.equal(document.getElementById('btn-check-update').textContent, 'Retry Install');
    assert.equal(document.getElementById('btn-check-update').disabled, false);
});

test('a failed launcher handoff keeps the exact tag and offers install retry', () => {
    mountUpdatePane();
    OSA.renderUpdateStatus({
        status: 'error',
        tag: 'v2.0.0',
        version: '2.0.0',
        launcher_managed: true,
        message: 'The launcher could not install the update',
        error: 'health check timed out'
    });
    assert.equal(OSA.pendingUpdateTag, 'v2.0.0');
    assert.equal(OSA._updateState.retryAction, 'install');
    assert.equal(document.getElementById('btn-check-update').textContent, 'Retry Install');
});

test('startup update toast is quiet and deduped per version', () => {
    mountUpdatePane();
    window.sessionStorage.clear();
    const messages = [];
    OSA.showToast = message => messages.push(message);
    OSA.renderUpdateStatus({ status: 'available', latest_version: '3.0.0', release_url: 'https://example.com/v3' });
    assert.equal(OSA.maybeShowStartupUpdateToast({ latest_version: '3.0.0' }), true);
    assert.equal(OSA.maybeShowStartupUpdateToast({ latest_version: '3.0.0' }), false);
    assert.equal(messages.length, 1);
    assert.match(messages[0], /3\.0\.0/);

    OSA.renderUpdateStatus({ status: 'ready', tag: 'v3.1.0', version: '3.1.0' });
    assert.equal(OSA.maybeShowStartupUpdateToast({ version: '3.1.0' }), true);
    assert.equal(messages.length, 2);
});
