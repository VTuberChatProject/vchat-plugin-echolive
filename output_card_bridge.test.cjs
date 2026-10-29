/* 按卡片隔离网络通道，并验证历史页面只接收定向消息。 */
'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const source = fs.readFileSync(require('node:path').join(__dirname, 'output_bridge.js'), 'utf8');
function page(card) {
    const context = {
        URL, console: {info() {}},
        location: {pathname: '/' + card + '.html', href: 'http://localhost:8080/' + card + '.html', host:'localhost:8080'},
        WebSocket: class { constructor(address) { this.address = address; } },
        BroadcastChannel: class { constructor(name) { this.name = name; } }
    };
    vm.createContext(context);
    vm.runInContext(`
        class EchoLiveBroadcastHistory {
            constructor() { this.received = []; this.echoLiveHistory = {
                send: value => this.received.push(value), clear: () => this.received.length = 0}; }
            getDataHistory(packet) { this.fallback = packet.action; }
        }
    `,context);
    vm.runInContext(source,context);
    return context;
}
const live = page('live');
const history = page('history');
assert.equal(vm.runInContext("new WebSocket('/ws').address", live),'http://localhost:8080/ws?card=live');
assert.equal(vm.runInContext("new WebSocket('ws://localhost:8080/ws?preview=1').address", history),
    'ws://localhost:8080/ws?preview=1&card=history');
assert.equal(vm.runInContext("new WebSocket('ws://other/ws').address", history),'ws://other/ws');
assert.equal(vm.runInContext("new BroadcastChannel('Echo-Live').name",live),'vchat:live:Echo-Live');
assert.equal(vm.runInContext("new BroadcastChannel('Echo-Live').name",history),'vchat:history:Echo-Live');
vm.runInContext(`
    const receiver = new EchoLiveBroadcastHistory();
    receiver.getDataHistory({action:'message_data',data:{username:'角色',messages:[{message:'甲'},{message:'乙'}]}});
    if (receiver.received.length !== 2 || receiver.received[1].message !== '乙') throw Error('history conversion');
    receiver.getDataHistory({action:'vchat_output_clear'});
    if (receiver.received.length) throw Error('history clear');
`,history);
console.log('EchoLive 输出卡片通道验证通过');
