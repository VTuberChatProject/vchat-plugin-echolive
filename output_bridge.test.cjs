/* 使用实际 Echo 打印器验证批次替换、迟到任务与清空的浏览器兼容行为。 */
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const timers = new Map();
let timerId = 0;
const context = {
    assert, console, timers,
    setTimeout: (fn, delay, ...args) => { const id = ++timerId; timers.set(id, () => fn(...args)); return id; },
    clearTimeout: id => timers.delete(id),
    setInterval: (fn, delay, ...args) => { const id = ++timerId; timers.set(id, () => fn(...args)); return id; },
    clearInterval: id => timers.delete(id),
    speechSynthesis: { cancel() {} },
};
vm.createContext(context);
vm.runInContext(fs.readFileSync(path.join(__dirname, '../../Assets/Frontend/Echo-Live/res/class/Echo.js'), 'utf8'), context);
vm.runInContext(`
class EchoLiveBroadcast {
    static API_NAME_ECHO_PRINTING = 'echo_printing';
    static API_NAME_ECHO_STATE_UPDATE = 'echo_state_update';
}
class EchoLiveBroadcastPortal {
    constructor(live) { this.echolive = live; this.sent = []; }
    sendData(data, action) { this.sent.push({ data, action }); }
    echoPrinting(username, message) { return this.sendData({ username, message }, 'echo_printing'); }
    echoStateUpdate(state) { return this.sendData({ state }, 'echo_state_update'); }
    getDataPortal(packet, listener = this) { if (packet.action === 'message_data') listener.echolive.send(packet.data); }
}
class EchoLive {
    constructor(echo) { this.echo = echo; this.pending = []; this.deferred = false; }
    clearTask() { this.pending = []; }
    clearDisplayHiddenWaitTimer() {}
    send(data) {
        if (this.echo.state !== 'stop') this.echo.stop();
        if (this.deferred) { this.pending.push(() => this.send(data)); return; }
        this.username = data.username;
        this.echo.sendList(data.messages);
    }
}
`, context);
vm.runInContext(fs.readFileSync(path.join(__dirname, 'output_bridge.js'), 'utf8'), context);
vm.runInContext(`
const echo = new Echo();
let groupIndex = 0, inRuby = false, inTypewriteEnd = false;
let messageActions = {};
const live = new EchoLive(echo);
const portal = new EchoLiveBroadcastPortal(live);
let output = '', speech = 0;
echo.on('clear', () => { output = ''; });
echo.on('print', text => { if (text) output += text; });
echo.on('next', msg => {
    portal.echoPrinting(live.username, msg.message);
    portal.echoStateUpdate('ready');
    setTimeout(() => speech++, 100);
});
echo.on('printEnd', () => portal.echoStateUpdate('stop'));
echo.on('stop', () => portal.echoStateUpdate('stop'));
const meta = generation => ({ message_id: 'message-' + generation,
    batch_id: 'batch-' + generation, run_id: 'run', generation });
const send = (generation, message = 'abc') => portal.getDataPortal({ action: 'message_data',
    data: { username: '角色', messages: [{ message }], vchat: meta(generation) } });
send(1);
assert.equal(portal.sent[0].data.vchat.batch_id, 'batch-1');
const oldSpeech = [...timers.values()][0];
echo.print(echo);
assert.equal(output, 'a');
groupIndex = 7; inRuby = true; echo.typewrite = 'input'; echo.ruby = true;
send(2, 'new');
assert.equal(output, '');
assert.equal(groupIndex, 0);
assert.equal(inRuby, false);
assert.equal(echo.typewrite, 'none');
assert.equal(echo.ruby, false);
assert.equal(portal.sent.filter(x => x.data.state === 'stop').length, 0);
oldSpeech();
assert.equal(speech, 0);
echo.print(echo); echo.print(echo); echo.print(echo);
assert.equal(output, 'new');
assert.equal(portal.sent.at(-1).data.vchat.batch_id, 'batch-2');
const count = portal.sent.length;
echo.event.printEnd();
assert.equal(portal.sent.length, count);
portal.getDataPortal({ action: 'vchat_output_clear', data: { vchat: meta(3) } });
assert.equal(output, '');
assert.equal(echo.state, 'stop');
assert.equal(echo.messageList.length, 0);
send(2, '过期');
assert.equal(output, '');
live.deferred = true;
send(4, '迟到');
const oldSend = live.pending[0];
live.deferred = false;
send(5, '当前');
oldSend();
assert.equal(echo.message, '当前');
portal.getDataPortal({ action: 'message_data', data: { username: '普通', messages: [{ message: 'ok' }] } });
assert.equal(portal.sent.at(-1).data.vchat, undefined);
send(6, '旧运行');
assert.equal(echo.message, 'ok');
`, context);
console.log('Echo 剧本兼容层验证通过');
