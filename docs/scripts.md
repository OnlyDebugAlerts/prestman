# Pre-request и Post-response scripts

Откройте запрос → **Scripts** → **Pre-request** или **Post-response**. Скрипты
пишутся на JavaScript с подсветкой синтаксиса. **Example** добавляет пример в
пустой редактор. **Save** (`⌘S` / `Ctrl+S`) сохраняет оба скрипта вместе с запросом;
при **Send** выполняются текущие правки, даже если они ещё не сохранены. При обычном
закрытии приложения все запросы и оба скрипта сохраняются автоматически.

Порядок выполнения: Pre-request → подстановка `{{variables}}` → HTTP-запрос →
Post-response. Скрипты выполняются в фоне во встроенном QuickJS; Node.js не нужен.

## Подготовка запроса

```javascript
pm.variables.set('timestamp', String(Date.now()));
pm.request.headers.upsert({
  key: 'X-Request-Time',
  value: '{{timestamp}}'
});
console.log('Sending', pm.request.method, String(pm.request.url));
```

Переменная доступна в URL, query, headers, Body и Authorization. Изменения
`pm.request` действуют на отправляемую копию: шаблон запроса в редакторе сохраняется.

## Проверка ответа и сохранение токена

```javascript
pm.test('Status is 200', () => {
  pm.response.to.have.status(200);
});

pm.test('Token exists', () => {
  pm.expect(pm.response.json()).to.have.property('token').that.is.a('string');
});

pm.environment.set('token', pm.response.json().token);
console.log('Response time:', pm.response.responseTime, 'ms');
```

В следующем запросе укажите `{{token}}`, например в Bearer Token. **Settings → Environment** открывает общее окружение приложения.
Глобальные переменные доступны в меню настроек внизу слева → **Globals**.
Environment, Globals и переменные коллекции сохраняются автоматически после
выполнения скриптов. `pm.variables.set` создаёт временную локальную переменную:
она доступна в pre-script, отправляемом запросе и post-script одного запуска.

Приоритет значений: local → environment → вложенная папка → родительские папки → collection → global. Отключённые
переменные не участвуют в поиске. `collectionVariables` сохраняются только для
запросов, добавленных в коллекцию.

## Дополнительный HTTP-запрос

`pm.sendRequest` принимает URL или объект `{url, method, header, body}`. Доступны
callback и Promise с `await`; тело дополнительного запроса поддерживает режим raw.

```javascript
const response = await pm.sendRequest({
  url: pm.variables.replaceIn('{{base_url}}/token'),
  method: 'POST',
  header: { 'Content-Type': 'application/json' },
  body: { mode: 'raw', raw: JSON.stringify({ username: 'demo' }) }
});
pm.environment.set('token', response.json().token);
```

```javascript
pm.sendRequest('https://example.com/health', (error, response) => {
  pm.test('Health check', () => {
    pm.expect(error).to.equal(null);
    pm.expect(response.code).to.equal(200);
  });
});
```

## Поддерживаемый API

| API | Возможности |
| --- | --- |
| `pm.variables`, `pm.environment`, `pm.globals`, `pm.collectionVariables` | `get`, `set`, `unset`, `has`, `clear`, `toObject`, `replaceIn` |
| `pm.variables.replaceIn` | Вложенные `{{name}}`, `$timestamp`, `$isoTimestamp`, `$guid`, `$randomUUID`, `$randomInt` |
| `pm.request` | Чтение/изменение `method`, `url`, `body.raw`; `body.update(string)` |
| `pm.request.headers` | `get`, `has`, `add`, `upsert`, `remove`, `clear`, `all`, `each`, `toJSON`, `toObject`; имена без учёта регистра |
| `pm.request.url` | `toString`, `getQueryString`, `addQueryParams`, `removeQueryParams`; `query` с теми же методами списка |
| `pm.response` | `code`, `status`, `responseTime` (мс), `size` (байты), `text()`, `json()`, `headers` |
| `pm.response.to.have` | `status(code)`, `header(name, value?)`, `body(text)`, `jsonBody(object?)` |
| `pm.response.to.be` | `success`, `error`, `ok`, `json` |
| `pm.test(name, fn)` | Синхронные проверки, Promise/async-функции и callback `done` |
| `pm.expect` | Chai 4.5.0: сравнения, `deep.equal`, свойства, типы, массивы, `.not` и другие стандартные assertions |
| `pm.visualizer` | `set(layout, data, options)`, `clear()`; HTML/CSS/JavaScript во вкладке Visualizer. [Подробнее](visualizer.md) |
| `pm.info` | `requestName`, `eventName` (`prerequest` / `test`) |
| `console` | `log`, `info`, `warn`, `error`, `debug` |
| `require('chai')` | Встроенная библиотека Chai |
| `CryptoJS`, `require('crypto-js')` | CryptoJS 4.2.0: WordArray, Hex/UTF-8/Base64/Base64URL, SHA/MD5, HMAC, PBKDF2, AES и другие алгоритмы библиотеки |
| `crypto.getRandomValues` | Заполнение целочисленного TypedArray криптографическими случайными байтами ОС (до 65536 байт за вызов) |

`pm.response.text()` возвращает исходный текст ответа; форматирование JSON в
интерфейсе не влияет на данные скрипта. Ответ доступен в post-script, включая
HTTP-статусы 4xx/5xx. При сетевой ошибке post-script не запускается.

## Результаты и ошибки

Внизу справа в панели ответа иконка **Tests** (галочка) открывает проверки
PASS/FAIL и ошибки скриптов, а **Console** (терминал) — сообщения с указанием этапа. Проваленная assertion внутри `pm.test`
не прерывает остальные тесты. Ошибки вне `pm.test` останавливают текущий скрипт.
Ошибка Pre-request предотвращает отправку основного запроса. Ошибка Post-response
сохраняет HTTP-ответ доступным во вкладках Body и Headers. Уже выполненные изменения
постоянных переменных сохраняются даже при последующей ошибке скрипта.

На один скрипт отведено 5 секунд, включая его дополнительные HTTP-запросы, и 64 МиБ
памяти JavaScript. Лимиты защищают интерфейс от зависаний. Консоль хранит до 500
сообщений по 8192 символа на этап; допускается до 1000 тестов на этап.

Это совместимая часть Postman Sandbox API для отдельных HTTP-запросов. Скрипты
коллекций/папок, Collection Runner, несколько именованных окружений, npm-пакеты
кроме Chai и CryptoJS, Node.js API, файловая система, `fetch`, таймеры, cookies API, Vault и
JSON Schema assertions пока не поддерживаются. Основной и дополнительные
HTTP-запросы используют HTTP-клиент приложения; дополнительные запросы выполняются
последовательно внутри фонового задания.

Ориентиры совместимости: [Postman Sandbox API](https://learning.postman.com/docs/tests-and-scripts/write-scripts/postman-sandbox-reference/overview/),
[переменные](https://learning.postman.com/docs/tests-and-scripts/write-scripts/postman-sandbox-reference/pm-variables/),
[запрос](https://learning.postman.com/docs/tests-and-scripts/write-scripts/postman-sandbox-reference/pm-request/),
[ответ](https://learning.postman.com/docs/tests-and-scripts/write-scripts/postman-sandbox-reference/pm-response/).
Chai 4.5.0 включён в `assets/vendor/chai.js` из npm-пакета `chai@4.5.0`;
лицензия MIT находится рядом в `chai-LICENSE`.

Для проверки без внешнего API используйте [готовые pre/post scripts и локальный сервер](../examples/scripts/README.md): ожидается **7/7 tests passed**.

`pm.environment.set(name, value)` создаёт или обновляет переменную общего окружения Prestman. После завершения скриптов изменения автоматически записываются в `environment_variables` локального файла `requests.json`. Открыть значения: **Settings → Environment**. `console.log()` отображается отдельно через иконку **Console** внизу справа в панели ответа.

## CryptoJS

Библиотека доступна в Pre-request и Post-response как глобальный `CryptoJS`
и через `const CryptoJS = require('crypto-js')`. Она встроена в приложение,
поэтому скачивание пакетов во время выполнения скрипта не требуется.

```javascript
const b64url = wordArray =>
  CryptoJS.enc.Base64.stringify(wordArray)
    .replace(/=/g, '')
    .replace(/\+/g, '-')
    .replace(/\//g, '_');

const signature = b64url(CryptoJS.HmacSHA256('message', 'key'));
pm.environment.set('signature', signature);
```

`CryptoJS.lib.WordArray.random` и генерация солей для AES используют генератор
случайных чисел ОС. `crypto.getRandomValues` предоставляется для этого механизма;
полный Web Crypto API (`crypto.subtle`) пока не реализован.

Используется CryptoJS 4.2.0 из `crypto-js@4.2.0`; исходник и лицензия MIT лежат
в `assets/vendor/crypto-js.js` и `assets/vendor/crypto-js-LICENSE`. Значения по
умолчанию алгоритмов могут отличаться от старых версий CryptoJS в Postman.
Для PBKDF2 явно задавайте `hasher`, `iterations` и `keySize`, если требуется
воспроизвести существующую схему вычисления ключа.

## Комментарии в JSON Body

В редакторе Body поддерживаются комментарии `// до конца строки` и
`/* многострочный комментарий */`. Форматирование сохраняет комментарии.
При подготовке JSON-запроса комментарии удаляются перед подстановкой переменных
и отправкой, поэтому `{{name}}` внутри закомментированных строк не требует
заданной переменной. Строковые значения и URL с `//` не меняются.

В Pre-request `pm.request.body.raw` содержит исходный шаблон с комментариями.
В Post-response — подготовленное тело отправленного запроса. Дополнительные
HTTP-запросы `pm.sendRequest` тоже удаляют комментарии из JSON-тел перед отправкой.
Для `Content-Type: text/plain` и других явно заданных не-JSON типов тело
передаётся без удаления комментариев. Незакрытый `/*` вызывает ошибку.

## Path variables

Path-переменные принадлежат запросу и редактируются в **Query → Path variables**.
Для URL `{{base_url}}/themes/:code` можно указать `code = {{theme_code}}`.
При отправке сначала выполняется pre-script, затем подстановка значений.

```javascript
console.log(pm.request.url.variables.get('code'));
pm.request.url.variables.set('code', '{{theme_code}}');
// Также поддерживаются upsert({key, value}), unset(name), remove(name), all().
```

Эти изменения действуют для текущей отправки. Они не меняют одноимённые
`pm.environment` / `pm.collectionVariables`, а исходные значения в редакторе
сохраняются. Пустое значение заменяет `:code` пустой строкой; выключенная
строка не подставляется. Параметры не заменяют порт, query и fragment URL.

Переменные папки доступны через `pm.variables.get` / `replaceIn` и при подстановке в запрос. `pm.collectionVariables` читает и изменяет переменные корневой коллекции. Их можно переопределить в папке; окружение имеет более высокий приоритет.
