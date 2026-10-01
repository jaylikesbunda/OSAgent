const assert = require('node:assert/strict');
const test = require('node:test');
test.before(async () => {
    const { Window } = await import('happy-dom');
    global.window = new Window(); global.document = window.document;
    global.OSA = window.OSA = {};
    require('../js/preview.js'); require('../js/filebrowser.js');
    OSA.getCurrentSession = () => ({id:'one'});
    OSA.getSessionToolEvents = () => [];
});
test.beforeEach(() => {
    document.body.innerHTML = '<div id="app-view"></div><div id="file-preview-panel"></div><div id="preview-resize-handle"></div><div id="file-browser-tree"></div><select id="file-browser-root"></select><input id="file-browser-filter"><button id="browser-files-tab"></button><button id="browser-changes-tab"></button><span id="browser-change-count"></span><div id="file-preview-tabs"></div><div id="file-preview-path"></div><div id="file-preview-modes"></div><div id="file-preview-body"></div><div id="file-preview-status"></div>';
    OSA._previewState.open = false; OSA.resetFileBrowser();
    OSA._fileBrowser.folders.set('', {entries:[{name:'src',path:'src',directory:true},{name:'index.html',path:'index.html',directory:false}]});
});
test('file tabs survive closing and source text cannot inject HTML', () => {
    OSA.openFilePreview('index.html', '<script>bad()</script>', {source:'workspace'});
    assert.equal(document.querySelector('#file-preview-body script'), null);
    assert.match(document.querySelector('#file-preview-body code').textContent, /<script>/);
    document.querySelector('.file-tab-close').click();
    OSA.openFilePreview('index.html', '<script>bad()</script>', {source:'workspace'});
    assert.match(document.querySelector('#file-preview-body code').textContent, /<script>/);
    for (let i=0;i<12;i++) OSA.openFilePreview(`file-${i}.txt`, 'hello');
    assert.equal(OSA._fileBrowser.tabs.size, 8);
});
test('folder expansion is lazy and preserves the current file tab', async () => {
    OSA.openFilePreview('index.html','hello');
    let requested;
    OSA.browserRequest = async path => { requested=path; return {kind:'directory',roots:['root'],entries:[{name:'app.js',path:'src/app.js',directory:false}]}; };
    document.querySelector('.file-tree-row').click();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(requested,'src');
    assert.match(document.getElementById('file-browser-tree').textContent,/app.js/);
    assert.equal(OSA._fileBrowser.selected,'0:index.html');
});
test('a stale file fetch cannot populate another chat', async () => {
    let resolveRequest;
    OSA.browserRequest = () => new Promise(resolve => {resolveRequest=resolve;});
    const opening = OSA.openBrowserFile('index.html');
    OSA._previewState.open=false; OSA.resetFileBrowser();
    resolveRequest({path:'index.html',content:'old chat'});
    await opening;
    assert.equal(OSA._fileBrowser.tabs.size,0);
});

test('closing the pane prevents an outstanding file request from reopening it', async () => {
    let resolveRequest;
    OSA.browserRequest = () => new Promise(resolve => {resolveRequest=resolve;});
    OSA._previewState.open = true;
    const opening = OSA.openBrowserFile('index.html');
    OSA.closeFilePreview();
    resolveRequest({path:'index.html',content:'late'});
    await opening;
    assert.equal(OSA._previewState.open,false);
    assert.equal(OSA._fileBrowser.tabs.size,0);
});
test('Changes keeps the original baseline and latest recorded contents', () => {
    OSA.getSessionToolEvents = () => [
        {tool_call_id:'1',tool_name:'edit_file',success:true,files:[{path:'a.txt',old_content:'a',new_content:'b'}]},
        {tool_call_id:'2',tool_name:'edit_file',success:true,files:[{path:'a.txt',old_content:'b',new_content:'c'}]},
    ];
    OSA.getToolDiffFiles = tool => tool.files;
    assert.deepEqual(OSA.browserChanges(),[{path:'a.txt',old_content:'a',new_content:'c'}]);
});
