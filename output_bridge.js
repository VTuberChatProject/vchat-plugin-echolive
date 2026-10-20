/* VChat 剧本输出兼容层：以批次隔离打印回执、延迟任务和清空操作。 */
(() => {
    'use strict';
    const states = new WeakMap();
    const echoes = new WeakMap();
    const copy = value => value ? { ...value } : null;
    const same = (a, b) => a && b && a.run_id === b.run_id &&
        a.generation === b.generation && a.batch_id === b.batch_id &&
        a.message_id === b.message_id;

    function state(live) {
        if (!states.has(live)) {
            const value = { active: null, latest: null, event: null, epoch: 0,
                transition: false, retired: new Set(), timers: new Set() };
            states.set(live, value);
            echoes.set(live.echo, value);
        }
        return states.get(live);
    }

    // 同一运行内只接受递增代次，跨运行保留退休集合以拒绝迟到消息。
    function accept(value, meta) {
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
        // 中途打断富文本时同步清理官方渲染器的分组和注音上下文。
        if (typeof groupIndex !== 'undefined') groupIndex = 0;
        if (typeof inRuby !== 'undefined') inRuby = false;
        if (typeof inTypewriteEnd !== 'undefined') inTypewriteEnd = false;
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

    // 只有 next/print 的同步调用链能发出当前批次回执，迟到事件保持隔离。
    for (const method of ['next', 'print']) {
        const original = Echo.prototype[method];
        Echo.prototype[method] = function(...args) {
            const value = echoes.get(method === 'print' ? (args[0] || this) : this);
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
