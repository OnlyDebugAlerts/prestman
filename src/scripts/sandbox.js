// Postman-compatible request scripting API. Chai is bundled separately under MIT.
(() => {
    const input = JSON.parse(__inputJSON);
    delete globalThis.__inputJSON;
    const mutations = [], tests = [], logs = [], pendingTests = [], pendingRequests = [];
    let visualization = null, visualizerChanged = false;
    const text = value => typeof value === "string" ? value : String(value);
    const stringify = value => {
        try { return typeof value === "string" ? value : JSON.stringify(value); }
        catch (_) { return String(value); }
    };
    const errorText = error => error && error.message ? error.message : String(error);
    const stores = {};
    function scope(name, rows) {
        const values = new Map(rows.filter(row => row.enabled).map(row => [row.name, row.value]));
        const api = {
            get: name => values.get(String(name)),
            has: name => values.has(String(name)),
            set(name, value) {
                name = String(name); value = text(value);
                if (!name.trim() || /[{}]/.test(name)) throw new Error("Enter a non-empty variable name without braces");
                if (api.scope === "collection" && input.scopes.collection_id === null) throw new Error("Move this request to a collection to save collection variables");
                values.set(name, value);
                if (api.scope !== "locals") mutations.push({scope: api.scope, name, value});
            },
            unset(name) {
                if (api.scope === "collection" && input.scopes.collection_id === null) throw new Error("This request does not belong to a collection");
                name = String(name); values.delete(name);
                if (api.scope !== "locals") mutations.push({scope: api.scope, name, value: null});
            },
            clear() { for (const name of [...values.keys()]) api.unset(name); },
            toObject: () => Object.fromEntries(values),
            replaceIn: value => replace(value, name => api.get(name)),
            scope: name,
        };
        stores[name] = api;
        return api;
    }
    const globals = scope("globals", input.scopes.globals);
    const collection = scope("collection", input.scopes.collection);
    const folder = scope("folder", input.scopes.folder || []);
    const environment = scope("environment", input.scopes.environment);
    const locals = scope("locals", Object.entries(input.scopes.locals).map(([name, value]) => ({name, value, enabled: true})));
    const getVariable = name => [locals, environment, folder, collection, globals].find(scope => scope.has(name))?.get(name);
    function replace(value, get = getVariable, stack = []) {
        return String(value).replace(/\{\{\s*([^{}]+?)\s*\}\}/g, (match, name) => {
            if (name === "$timestamp") return String(Math.floor(Date.now() / 1000));
            if (name === "$isoTimestamp") return new Date().toISOString();
            if (name === "$randomInt") return String(Math.floor(Math.random() * 1001));
            if (name === "$guid" || name === "$randomUUID") return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, c => {
                const n = Math.floor(Math.random() * 16); return (c === "x" ? n : (n & 3) | 8).toString(16);
            });
            const found = get(name);
            if (found === undefined) return match;
            if (stack.includes(name) || stack.length >= 32) throw new Error("Circular variable reference: " + name);
            return replace(found, get, [...stack, name]);
        });
    }
    function list(initial, insensitive = false, changed = () => {}) {
        let rows = initial.map(row => ({key: String(row.key ?? row.name), value: text(row.value), disabled: row.disabled === true || row.enabled === false}));
        const key = value => insensitive ? String(value).toLowerCase() : String(value);
        const api = {
            get: name => rows.find(row => !row.disabled && key(row.key) === key(name))?.value,
            has: name => rows.some(row => !row.disabled && key(row.key) === key(name)),
            add(row) { rows.push({key: String(row.key ?? row.name), value: text(row.value), disabled: !!row.disabled}); changed(); },
            upsert(row) {
                const found = rows.find(item => key(item.key) === key(row.key ?? row.name));
                if (found) Object.assign(found, {value: text(row.value), disabled: !!row.disabled});
                else api.add(row);
                changed();
            },
            remove(name) { rows = rows.filter(row => key(row.key) !== key(name)); changed(); },
            clear() { rows = []; changed(); },
            all: () => rows.map(row => ({...row})),
            toJSON: () => rows.map(row => ({...row})),
            each: callback => rows.forEach(row => callback({...row})),
            toObject: () => Object.fromEntries(rows.filter(row => !row.disabled).map(row => [row.key, row.value])),
        };
        return api;
    }
    const encode = value => encodeURIComponent(value).replace(/%7B%7B.*?%7D%7D/g, value => decodeURIComponent(value));
    function urlObject(raw, initial, pathVariables) {
        let dirty = false;
        const query = list(initial, false, () => { dirty = true; });
        const variables = list(pathVariables);
        variables.set = (name, value) => variables.upsert({key: name, value});
        variables.unset = name => variables.remove(name);
        const queryString = () => query.all().filter(row => !row.disabled && (row.key || row.value)).map(row => `${encode(row.key)}=${encode(row.value)}`).join("&");
        return {
            query,
            variables,
            toString() {
                if (!dirty) return raw;
                const hash = raw.indexOf("#");
                const fragment = hash < 0 ? "" : raw.slice(hash);
                const base = (hash < 0 ? raw : raw.slice(0, hash)).split("?")[0];
                return base + (queryString() ? "?" + queryString() : "") + fragment;
            },
            getQueryString: queryString,
            addQueryParams(values) { (Array.isArray(values) ? values : [values]).forEach(value => query.add(value)); },
            removeQueryParams(name) { query.remove(name); },
        };
    }
    function messages(values) {
        const rows = values.map(data => ({data, timestamp: new Date()}));
        return {
            idx: index => rows[index],
            count: () => rows.length,
            all: () => rows.slice(),
            each: callback => rows.forEach(callback),
            filter: pattern => rows.filter(row => {
                if (typeof pattern === "function") return pattern(row);
                try { chai.expect(row).to.deep.include(pattern); return true; } catch (_) { return false; }
            }),
            to: {
                include(expected) {
                    chai.expect(values.some(value => {
                        try { chai.expect(value).to.deep.include(expected); return true; } catch (_) { return false; }
                    })).to.equal(true);
                },
                have: {
                    property(name, ...expected) {
                        values.forEach(value => chai.expect(value).to.deep.nested.property(name, ...expected));
                    },
                },
            },
        };
    }
    function responseObject(data) {
        if (!data) return undefined;
        const response = {...data, headers: list(data.headers, true), text: () => data.body, json: () => JSON.parse(data.body)};
        if (data.grpc) {
            response.statusCode = data.code;
            response.metadata = response.headers;
            response.trailers = list(data.grpc.trailers, true);
            response.messages = messages(data.grpc.messages);
        }
        const checks = {
            status(expected) { chai.expect(response.code).to.equal(expected); },
            header(name, expected) { chai.expect(response.headers.has(name)).to.equal(true); if (expected !== undefined) chai.expect(response.headers.get(name)).to.equal(expected); },
            body(expected) { chai.expect(response.text()).to.equal(expected); },
            jsonBody(expected) { const actual = response.json(); if (expected !== undefined) chai.expect(actual).to.deep.include(expected); },
        };
        for (const [name, predicate] of Object.entries({success: () => data.grpc ? response.code === 0 : response.code >= 200 && response.code < 300, error: () => data.grpc ? response.code !== 0 : response.code >= 400, ok: () => response.code === (data.grpc ? 0 : 200), json: () => {response.json(); return true;}})) {
            Object.defineProperty(checks, name, {get() { chai.expect(predicate()).to.equal(true); return checks; }});
        }
        if (data.grpc) {
            checks.statusCode = checks.status;
            checks.metadata = checks.header;
            checks.trailer = (name, expected) => {
                chai.expect(response.trailers.has(name)).to.equal(true);
                if (expected !== undefined) chai.expect(response.trailers.get(name)).to.equal(expected);
            };
            checks.message = expected => chai.expect(data.grpc.messages).to.deep.include(expected);
            checks.responseTime = chai.expect(response.responseTime).to.be;
        }
        response.to = {have: checks, be: checks};
        return response;
    }
    const request = {
        method: input.request.method,
        url: urlObject(input.request.url, input.request.query || [], input.request.path_variables || []),
        headers: list(input.request.headers, true),
        body: {mode: "raw", raw: input.request.body, update(value) { this.raw = typeof value === "object" ? value.raw : text(value); }},
    };
    if (input.request.grpc) {
        const body = request.body.raw;
        let data;
        try { data = JSON.parse(body); } catch (_) { data = body; }
        request.methodPath = request.method.replace("/", ".");
        request.metadata = request.headers;
        request.messages = messages(data === "" ? [] : [data]);
    }
    if (input.request.graphql) {
        Object.defineProperty(request, "query", {
            get: () => request.body.raw,
            set: value => { request.body.raw = text(value); },
        });
        request.variables = input.request.graphql.variables;
        request.operationName = input.request.graphql.operation_name;
    }
    function exportRequest() {
        const exported = {...input.request, method: request.method, url: String(request.url),
            headers: request.headers.all().map(row => ({name: row.key, value: row.value, enabled: !row.disabled})),
            body: request.body.raw, query: [],
            path_variables: request.url.variables ? request.url.variables.all().map(row => ({name: row.key, value: row.value, enabled: !row.disabled})) : input.request.path_variables || []};
        if (input.request.graphql) {
            exported.graphql = {
                variables: typeof request.variables === "string" ? request.variables : JSON.stringify(request.variables),
                operation_name: text(request.operationName),
            };
        }
        return exported;
    }
    const isMessage = input.phase === "On message";
    const pm = {
        visualizer: {
            set(layout, data = {}, options = {}) {
                if (typeof layout !== "string") throw new TypeError("Visualizer layout must be an HTML template string");
                const snapshot = JSON.parse(JSON.stringify(data));
                const html = Handlebars.compile(layout, options)(snapshot);
                visualization = {html, data: snapshot};
                visualizerChanged = true;
            },
            clear() { visualization = null; visualizerChanged = true; },
        },
        globals, collectionVariables: collection, environment,
        variables: {...locals, get: getVariable, has: name => getVariable(name) !== undefined,
            replaceIn: value => replace(value), toObject: () => Object.assign({}, globals.toObject(), collection.toObject(), folder.toObject(), environment.toObject(), locals.toObject())},
        request: isMessage ? undefined : request, response: responseObject(input.response),
        message: isMessage ? {data: input.message, timestamp: new Date()} : undefined,
        info: {requestName: input.request.name || input.request.url, eventName: input.request.grpc
            ? (isMessage ? "onIncomingMessage" : input.phase === "Pre-request" ? "beforeInvoke" : "afterResponse")
            : input.request.graphql
                ? (input.phase === "Pre-request" ? "beforeQuery" : "afterResponse")
                : (input.phase === "Pre-request" ? "prerequest" : "test")},
        expect: chai.expect,
        test(name, callback) {
            if (tests.length >= 1000) throw new Error("Too many tests (maximum 1000 per script)");
            const result = {name: String(name), passed: false, error: null, phase: input.phase};
            tests.push(result);
            try {
                if (callback.length) {
                    const completion = new Promise((resolve, reject) => {
                        const returned = callback(error => error ? reject(error) : resolve());
                        if (returned && typeof returned.then === "function") returned.catch(reject);
                    });
                    pendingTests.push(completion.then(() => {result.passed = true;}).catch(error => {result.error = errorText(error);}));
                } else {
                    const returned = callback();
                    pendingTests.push(Promise.resolve(returned).then(() => {result.passed = true;}).catch(error => {result.error = errorText(error);}));
                }
            } catch (error) { result.error = errorText(error); }
        },
        sendRequest(config, callback) {
            let response, error;
            try {
                if (typeof config === "string") config = {url: config};
                const headers = config.header || config.headers || [];
                const rows = Array.isArray(headers) ? headers : Object.entries(headers).map(([key, value]) => ({key, value}));
                if (config.body?.mode && config.body.mode !== "raw") throw new Error("pm.sendRequest supports raw bodies only");
                const outgoing = {name: "", method: config.method || "GET", url: replace(String(config.url)),
                    headers: rows.map(row => ({name: replace(row.key ?? row.name), value: replace(row.value), enabled: !row.disabled})),
                    body: replace(typeof config.body === "string" ? config.body : config.body?.raw || "")};
                const result = JSON.parse(__sendRequest(JSON.stringify(outgoing)));
                if (result.error) throw new Error(result.error);
                response = responseObject(result.response);
            } catch (caught) { error = caught; }
            if (callback) {
                const returned = callback(error || null, response);
                if (returned && typeof returned.then === "function") pendingRequests.push(returned);
                return;
            }
            const promise = error ? Promise.reject(error) : Promise.resolve(response);
            return promise;
        },
    };
    pm.test.index = () => tests.length;
    globalThis.pm = pm;
    globalThis.console = Object.fromEntries(["log", "info", "warn", "error", "debug"].map(level => [level, (...values) => {
        if (logs.length < 500) logs.push({level, message: values.map(stringify).join(" ").slice(0, 8192), phase: input.phase});
    }]));
    globalThis.require = name => {
        if (name === "chai") return chai;
        if (name === "crypto-js") return CryptoJS;
        throw new Error(`Unsupported module: ${name}`);
    };
    globalThis.__waitForTests = async () => { await Promise.all(pendingRequests); await Promise.all(pendingTests); };
    globalThis.__exportJSON = () => JSON.stringify({request: exportRequest(), locals: locals.toObject(), mutations,
        tests: tests.map(test => !test.passed && !test.error ? {...test, error: 'Test did not complete'} : test), logs,
        visualization, visualizer_changed: visualizerChanged});
})();
