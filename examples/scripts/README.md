# Проверка pre/post scripts без внешних сервисов

1. Из корня проекта запустите локальный сервер:

   ```bash
   python3 examples/scripts/server.py
   ```

2. Создайте в Prestman новый запрос с URL:

   ```text
   http://127.0.0.1:8765/check?id={{test_id}}
   ```

3. Во вкладку **Scripts → Pre-request** вставьте содержимое [pre-request.js](pre-request.js),
   в **Post-response** — [post-response.js](post-response.js).
4. Нажмите **Send**. Метод POST, JSON-тело и заголовки установит pre-script.

Ожидаемый результат: **200 OK**, по иконке **Tests** внизу справа — **7/7 tests passed**,
по иконке **Console** рядом — сообщения `Pre-request script executed` и
`Post-response script executed`. **Settings → Environment**: переменная `script_check` должна содержать `post-ok`.

Проверяются оба этапа, изменение метода/заголовка/тела, подстановка `{{test_id}}`
в URL, получение JSON-ответа, assertions и сохранение переменной окружения.

Для проверки отображения ошибок замените в post-script ожидаемый статус `200`
на `201` и снова нажмите Send. Должно стать **6/7 tests passed**, с красным
результатом проверки статуса. HTTP-ответ останется доступным во вкладке Body.
Остановите сервер сочетанием `Ctrl+C`.

Для проверки визуализации замените post-script содержимым [visualizer.js](visualizer.js),
отправьте запрос и откройте **Visualizer** после **Body** в панели ответа.
Должны появиться таблица заголовков, кнопка Show / hide request body и график.
Нажатие кнопки скрывает и показывает JSON-тело запроса.
