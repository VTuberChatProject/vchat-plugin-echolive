/* 输出页面按卡片隔离 WebSocket 与跨标签广播，资源目录保持原样。 */
(() => {
    'use strict';
    if (typeof location === 'undefined') return;
    const card = /\/history\.html$/.test(location.pathname) ? 'history' : 'live';
    const NativeWebSocket = globalThis.WebSocket;
    if (typeof NativeWebSocket === 'function') {
        globalThis.WebSocket = class extends NativeWebSocket {
            constructor(address, protocols) {
                const url = new URL(address, location.href);
                if (url.host === location.host) url.searchParams.set('card', card);
                super(url.href, ...(protocols === undefined ? [] : [protocols]));
            }
        };
    }
    const NativeChannel = globalThis.BroadcastChannel;
    if (typeof NativeChannel === 'function') {
        globalThis.BroadcastChannel = class extends NativeChannel {
            constructor(name) { super(`vchat:${card}:${name}`); }
        };
    }
    if (card === 'history' && typeof EchoLiveBroadcastHistory !== 'undefined') {
        const receive = EchoLiveBroadcastHistory.prototype.getDataHistory;
        EchoLiveBroadcastHistory.prototype.getDataHistory = function(packet, listener = this) {
            if (packet.action === 'message_data') {
                for (const item of packet.data?.messages || []) {
                    listener.echoLiveHistory.send({ username: packet.data?.username, message:item.message });
                }
                return;
            }
            if (packet.action === 'vchat_output_clear') { listener.echoLiveHistory.clear(); return; }
            return receive.call(this, packet, listener);
        };
    }
})();
/* VChat 剧本输出兼容层：以批次隔离打印回执、延迟任务和清空操作。 */
(() => {
    'use strict';
    // 资源目录可能被用户更新；安装桥接前确认这一代页面具有完整控制接口。
    if (typeof EchoLiveBroadcastPortal === 'undefined' || typeof EchoLive === 'undefined' ||
        typeof Echo === 'undefined' ||
        !['getDataPortal', 'echoPrinting', 'echoStateUpdate'].every(key => typeof EchoLiveBroadcastPortal.prototype[key] === 'function') ||
        !['clearTask', 'clearDisplayHiddenWaitTimer', 'send'].every(key => typeof EchoLive.prototype[key] === 'function') ||
        !['createSubTrack', 'next', 'print', 'on', 'stop'].every(key => typeof Echo.prototype[key] === 'function')) {
        console.info('VChat：当前 EchoLive 页面使用基础输出模式，批次控制能力受限。');
        return;
    }
    const states = new WeakMap();
    const echoes = new WeakMap();
    const copy = value => value ? { ...value } : null;
    const same = (a, b) => a && b && a.run_id === b.run_id &&
        a.generation === b.generation && a.batch_id === b.batch_id &&
        a.message_id === b.message_id;

    function state(live) {
        if (!states.has(live)) {
            const value = { active: null, latest: null, event: null, epoch: 0,
                transition: false, retired: new Set(), batches: new Set(), timers: new Set() };
            states.set(live, value);
            echoes.set(live.echo, value);
        }
        return states.get(live);
    }

    // 同一运行内只接受递增代次，跨运行保留退休集合以拒绝迟到消息。
    function accept(value, meta) {
        if (meta && meta.run_id === undefined && typeof meta.frontend_id === 'string' &&
            typeof meta.batch_id === 'string' && typeof meta.message_id === 'string') {
            if (value.batches.has(meta.batch_id)) return false;
            value.batches.add(meta.batch_id);
            if (value.batches.size > 1024) value.batches.delete(value.batches.values().next().value);
            if (value.latest?.run_id) value.retired.add(value.latest.run_id);
            value.latest = copy(meta);
            return true;
        }
        if (!meta || typeof meta.run_id !== 'string' ||
            typeof meta.batch_id !== 'string' || typeof meta.message_id !== 'string' ||
            !Number.isSafeInteger(meta.generation) || meta.generation < 0 ||
            value.retired.has(meta.run_id)) return false;
        if (value.latest?.run_id === meta.run_id &&
            meta.generation <= value.latest.generation) return false;
        if (value.latest && value.latest.run_id !== meta.run_id) {
            value.retired.add(value.latest.run_id);
        }
        value.latest = copy(meta);
        return true;
    }

    // 先退休旧批次再停止打印，stop 的同步回调始终处于隔离区间。
    function retire(live, value, clear) {
        value.transition = true;
        value.active = null;
        value.epoch++;
        for (const timer of value.timers) globalThis.clearTimeout(timer);
        value.timers.clear();
        live.clearTask();
        live.clearDisplayHiddenWaitTimer();
        live.echo.stop();
        live.echo.messageList = [];
        live.echo.typewrite = 'none';
        live.echo.ruby = false;
        // 1.9 的渲染上下文按轨道保存，下次 next 会重新建立全部轨道。
        if (typeof echoTrack !== 'undefined') echoTrack = {};
        if (typeof messageActions !== 'undefined') messageActions.printEnd = undefined;
        if (globalThis.speechSynthesis) globalThis.speechSynthesis.cancel();
        if (clear) {
            live.echo.clear();
            live.username = '';
            if (typeof globalThis.$ === 'function') $('#echo-live .name').empty();
        }
    }

    const portal = EchoLiveBroadcastPortal.prototype;
    const receive = portal.getDataPortal;
    portal.getDataPortal = function(packet, listener = this) {
        const live = listener.echolive;
        const value = state(live);
        const meta = packet.data?.vchat;
        if (packet.action === 'vchat_output_clear') {
            if (accept(value, meta)) retire(live, value, true);
            return;
        }
        if (packet.action === 'message_data') {
            if (meta) {
                if (!accept(value, meta)) return;
                retire(live, value, false);
            } else if (value.latest) {
                value.retired.add(value.latest.run_id);
                value.latest = null;
                retire(live, value, false);
                value.transition = false;
            }
        }
        return receive.call(this, packet, listener);
    };

    // 延迟显示动画会再次调用 send；原 payload 的代次必须仍属于活动节点。
    const send = EchoLive.prototype.send;
    EchoLive.prototype.send = function(data = {}) {
        const value = state(this);
        const meta = data.vchat;
        if (meta) {
            if (!same(value.latest, meta)) return;
            value.active = copy(meta);
            value.transition = false;
        }
        return send.call(this, data);
    };

    // 最后结束的副轨道会触发主轨道 printEnd，共享批次上下文以关联完成回执。
    const createSubTrack = Echo.prototype.createSubTrack;
    Echo.prototype.createSubTrack = function(...args) {
        const track = createSubTrack.apply(this, args);
        const value = echoes.get(this);
        if (track && value) echoes.set(track, value);
        return track;
    };

    // 只有 next/print 的同步调用链能发出当前批次回执，迟到事件保持隔离。
    for (const method of ['next', 'print']) {
        const original = Echo.prototype[method];
        Echo.prototype[method] = function(...args) {
            const value = echoes.get(this);
            if (!value) return original.apply(this, args);
            const previous = value.event;
            value.event = copy(value.active);
            try { return original.apply(this, args); }
            finally { value.event = previous; }
        };
    }

    const printing = portal.echoPrinting;
    portal.echoPrinting = function(username, message, target) {
        const value = state(this.echolive);
        if (value.transition) return;
        if (!value.active) return printing.call(this, username, message, target);
        if (!same(value.event, value.active)) return;
        return this.sendData({ username, message, vchat: copy(value.event) },
            EchoLiveBroadcast.API_NAME_ECHO_PRINTING, target);
    };
    const update = portal.echoStateUpdate;
    portal.echoStateUpdate = function(status, count = 0, target) {
        const value = state(this.echolive);
        if (value.transition) return;
        if (!value.active) return update.call(this, status, count, target);
        if (!same(value.event, value.active)) return;
        return this.sendData({ state: status, messagesCount: count,
            vchat: copy(value.event) }, EchoLiveBroadcast.API_NAME_ECHO_STATE_UPDATE, target);
    };

    // 官方 next 回调可能延迟启动语音；在该同步回调内记录定时器并校验代次。
    const on = Echo.prototype.on;
    Echo.prototype.on = function(name, callback) {
        if (name !== 'next' || typeof callback !== 'function') return on.call(this, name, callback);
        const echo = this;
        return on.call(this, name, function(...args) {
            const value = echoes.get(echo);
            if (!value) return callback.apply(this, args);
            const epoch = value.epoch;
            const timeout = globalThis.setTimeout;
            globalThis.setTimeout = function(action, delay, ...parameters) {
                const timer = timeout.call(globalThis, () => {
                    value.timers.delete(timer);
                    if (epoch === value.epoch && typeof action === 'function') action(...parameters);
                }, delay);
                value.timers.add(timer);
                return timer;
            };
            try { return callback.apply(this, args); }
            finally { globalThis.setTimeout = timeout; }
        });
    };
})();
