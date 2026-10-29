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
context.window = context;
vm.createContext(context);
const resourceRoot = process.env.VCHAT_ECHOLIVE_RESOURCE_ROOT;
assert.ok(resourceRoot, '请设置 VCHAT_ECHOLIVE_RESOURCE_ROOT 为实际 Echo-Live 资源目录');
vm.runInContext(fs.readFileSync(path.join(resourceRoot, 'res/class/Echo.js'), 'utf8'), context);
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
let echoTrack = {};
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
echoTrack.main = { groupIndex: 7, inRuby: true };
echo.typewrite = 'input'; echo.ruby = true;
send(2, 'new');
assert.equal(output, '');
assert.deepEqual(echoTrack, {});
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

// 主轨与副轨按任意顺序完成，整条消息只产生一次带批次标识的完成回执。
function multitrack(generation, main, tracks) {
    const printer = new Echo();
    const live = new EchoLive(printer);
    const portal = new EchoLiveBroadcastPortal(live);
    const output = {};
    printer.on('print', (text, track) => {
        if (text) output[track] = (output[track] || '') + text;
    });
    printer.on('next', msg => portal.echoPrinting(live.username, msg.message));
    printer.on('printEnd', () => portal.echoStateUpdate('stop'));
    printer.on('stop', () => portal.echoStateUpdate('stop'));
    const deliver = (generation, main, tracks) => portal.getDataPortal({ action: 'message_data',
        data: { username: '多轨角色', messages: [{ message: main,
            track: tracks.map(message => ({ message })) }], vchat: meta(generation) } });
    deliver(generation, main, tracks);
    return { printer, portal, output, deliver };
}
function finish(printer) {
    for (let i = 0; i < 100 && ['ready', 'play'].includes(printer.state); i++) printer.print();
    assert.ok(['stop', 'wait_subtrack'].includes(printer.state));
}
const multi = multitrack(1, 'a', ['long', 'xy']);
const sub0 = multi.printer.subTrack.get('sub_0');
const sub1 = multi.printer.subTrack.get('sub_1');
finish(multi.printer);
assert.equal(multi.printer.state, 'wait_subtrack');
finish(sub1);
assert.equal(multi.portal.sent.filter(x => x.data.state === 'stop').length, 0);
finish(sub0);
assert.deepEqual(multi.output, { main: 'a', sub_1: 'xy', sub_0: 'long' });
assert.equal(multi.printer.state, 'stop');
assert.equal(multi.portal.sent.filter(x => x.data.state === 'stop').length, 1);
assert.equal(multi.portal.sent.at(-1).data.vchat.batch_id, 'batch-1');
assert.equal(multi.printer.subTrack.size, 0);

const mainLast = multitrack(1, 'long', ['a']);
finish(mainLast.printer.subTrack.get('sub_0'));
assert.equal(mainLast.portal.sent.filter(x => x.data.state === 'stop').length, 0);
finish(mainLast.printer);
assert.equal(mainLast.portal.sent.filter(x => x.data.state === 'stop').length, 1);
assert.equal(mainLast.portal.sent.at(-1).data.vchat.batch_id, 'batch-1');

// 替换和清空销毁旧副轨，已取出的定时回调也无法输出或发出新批次回执。
const replaced = multitrack(1, 'old', ['stale']);
const retiredTrack = replaced.printer.subTrack.get('sub_0');
const stalePrint = timers.get(retiredTrack.timer);
retiredTrack.print();
replaced.deliver(2, 'new', ['current']);
const sentBeforeStale = replaced.portal.sent.length;
const outputBeforeStale = { ...replaced.output };
stalePrint();
assert.equal(retiredTrack.destroyed, true);
assert.deepEqual(replaced.output, outputBeforeStale);
assert.equal(replaced.portal.sent.length, sentBeforeStale);
finish(replaced.printer);
finish(replaced.printer.subTrack.get('sub_0'));
assert.equal(replaced.portal.sent.at(-1).data.vchat.batch_id, 'batch-2');
assert.equal(replaced.portal.sent.filter(x => x.data.state === 'stop').length, 1);

replaced.deliver(3, 'clear', ['pending']);
const clearedTrack = replaced.printer.subTrack.get('sub_0');
const clearedPrint = timers.get(clearedTrack.timer);
replaced.portal.getDataPortal({ action: 'vchat_output_clear', data: { vchat: meta(4) } });
const sentBeforeClear = replaced.portal.sent.length;
const outputBeforeClear = { ...replaced.output };
clearedPrint();
assert.equal(clearedTrack.destroyed, true);
assert.equal(replaced.printer.subTrack.size, 0);
assert.deepEqual(replaced.output, outputBeforeClear);
assert.equal(replaced.portal.sent.length, sentBeforeClear);

// 普通按卡消息携带完整身份，去重批次后仍只发出一次完成回执。
const ordinary = {frontend_id:'card:live',plugin_id:'builtin.echolive',card_id:'live',
    message_id:'ordinary-message',batch_id:'ordinary-batch'};
portal.getDataPortal({action:'message_data',data:{username:'普通',messages:[{message:'定向'}],vchat:ordinary}});
finish(echo);
assert.equal(output,'定向');
assert.equal(portal.sent.at(-1).data.vchat.card_id,'live');
assert.equal(portal.sent.at(-1).data.vchat.batch_id,'ordinary-batch');
const ordinaryCount = portal.sent.length;
portal.getDataPortal({action:'message_data',data:{username:'重复',messages:[{message:'重复'}],vchat:ordinary}});
assert.equal(portal.sent.length,ordinaryCount);
assert.equal(output,'定向');
`, context);
console.log('Echo 剧本兼容层验证通过');
