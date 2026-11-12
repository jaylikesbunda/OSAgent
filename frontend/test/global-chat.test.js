const assert = require('node:assert/strict');
const test = require('node:test');

test('global chat shows no workspace and never requests or changes one', async () => {
    const { Window } = await import('happy-dom');
    global.window = new Window();
    global.document = window.document;
    global.OSA = window.OSA = {};
    document.body.innerHTML = '<span id="context-trigger-label"></span><span id="ctx-ws-active-label"></span><div id="workspace-menu-list"></div>';
    OSA.getCurrentSession = () => ({ id: 'global-coordinator', metadata: { mode: 'global_coordinator' } });
    OSA.getWorkspaceState = () => ({ activeWorkspace: 'other', workspaces: [] });
    OSA.fetchWithAuth = () => { throw new Error('global chat should not request a workspace'); };
    require('../js/workspace.js');

    await OSA.loadSessionWorkspace();
    assert.equal(document.getElementById('context-trigger-label').textContent, 'Global chat');
    assert.equal(document.getElementById('ctx-ws-active-label').textContent, 'No workspace');
    assert.match(document.getElementById('workspace-menu-list').textContent, /no workspace/i);
    OSA.updateWorkspaceChip('other', '/some/project');
    assert.equal(document.getElementById('context-trigger-label').textContent, 'Global chat');
    await OSA.selectWorkspaceFromMenu('other');
});
