const assert = require('node:assert/strict');
const test = require('node:test');

test.before(async () => {
    const { Window } = await import('happy-dom');
    const window = new Window({ url: 'http://localhost/' });
    global.window = window;
    global.document = window.document;
    global.navigator = window.navigator;
    global.localStorage = window.localStorage;
    global.OSA = window.OSA = {};

    require('../js/state.js');
    require('../js/utils.js');
    require('../js/voice.js');
});

test('browser speech and voice output move to the offline voice only in the desktop window once it is installed', () => {
    const config = { stt_provider: 'browser', tts_provider: 'browser' };

    OSA.isTauriWebview = () => true;
    OSA.sherpaInstalled = true;
    assert.equal(OSA.effectiveSttProvider(config), 'sherpa-local');
    assert.equal(OSA.effectiveTtsProvider(config), 'sherpa-local');

    OSA.isTauriWebview = () => false;
    assert.equal(OSA.effectiveSttProvider(config), 'browser');
    assert.equal(OSA.effectiveTtsProvider(config), 'browser');

    OSA.isTauriWebview = () => true;
    OSA.sherpaInstalled = false;
    assert.equal(OSA.effectiveSttProvider(config), 'browser');
    assert.equal(OSA.effectiveTtsProvider(config), 'browser');
});

test('an engine the user chose is never swapped for the offline voice', () => {
    OSA.isTauriWebview = () => true;
    OSA.sherpaInstalled = true;
    assert.equal(OSA.effectiveSttProvider({ stt_provider: 'whisper-local' }), 'whisper-local');
    assert.equal(OSA.effectiveTtsProvider({ tts_provider: 'piper-local' }), 'piper-local');
    assert.equal(OSA.effectiveSttProvider({ stt_provider: 'sherpa-local' }), 'sherpa-local');
    assert.equal(OSA.effectiveSttProvider({ stt_provider: 'whisper' }), 'whisper-local');
});

test('local speech names say which engine answered', () => {
    assert.equal(OSA.localSttName('sherpa-local'), 'NVIDIA Parakeet TDT 0.6B v2');
    assert.equal(OSA.localSttName('whisper-local'), 'Local Whisper');
    assert.equal(OSA.isLocalSttProvider('sherpa-local'), true);
    assert.equal(OSA.isLocalSttProvider('browser'), false);
});
