# Visualizer

Вкладка **Visualizer** находится сразу после **Body** в панели ответа. Добавьте
в **Scripts → Post-response** HTML-шаблон и данные:

```javascript
const template = `
  <style>td, th { padding: 8px; border-bottom: 1px solid #ddd; }</style>
  <table>
    <tr><th>Name</th><th>Email</th></tr>
    {{#each users}}
      <tr><td>{{name}}</td><td>{{email}}</td></tr>
    {{/each}}
  </table>
`;
pm.visualizer.set(template, { users: pm.response.json() });
```

Пример ожидает массив объектов с `name` и `email`. Для проверки без внешнего API
используйте [локальный сервер](../examples/scripts/README.md) и
[готовый Visualizer-скрипт](../examples/scripts/visualizer.js): он показывает
таблицу, интерактивную кнопку и график на canvas.

## API

`pm.visualizer.set(layout, data?, options?)` компилирует Handlebars 4.7.8 и
устанавливает визуализацию текущего запуска. Поддерживаются `#each`, `#if`,
`#unless`, `#with`, `@index`, `@key`, пути к данным, HTML-экранирование и параметры
`Handlebars.compile`, например `{ noEscape: true }`. Последний вызов `set` заменяет
предыдущий. `pm.visualizer.clear()` удаляет визуализацию.

Можно вызвать `set` в Pre-request или Post-response. Визуализация pre-script
сохраняется, если post-script её не изменяет. Каждый новый Send начинает с пустой
визуализации; между разными запросами результат не смешивается. Ошибка скрипта
после успешного `set` не уничтожает уже подготовленный результат.

Внутри HTML-шаблона работают CSS, DOM JavaScript, canvas, `<style>`, `<script>`,
внешние `<script src="https://…">` и `<link rel="stylesheet" href="https://…">`.
Это позволяет подключать Chart.js, D3 и другие браузерные библиотеки. Для доступа
к переданным данным используйте API внутри `<script>` шаблона:

```html
<pre id="result"></pre>
<script>
pm.getData(function (error, data) {
  document.getElementById('result').textContent = JSON.stringify(data, null, 2);
});
</script>
```

В шаблоне доступен `pm.getData(callback)`; объект `pm` со скриптовыми переменными
и HTTP-функциями в страницу не передаётся. JavaScript и CSS выполняются в
изолированном iframe. Навигация основной страницы, всплывающие окна, доступ к
интерфейсу приложения и загрузка файлов отключены. Внешним библиотекам нужен
интернет; браузерные CORS-ограничения продолжают действовать. Ошибки JavaScript
и загрузки скриптов отображаются в самом Visualizer.

## Платформы и проверка

Используется системный WebView: WKWebView на macOS, WebView2 на Windows и
WebKitGTK на Linux. В Linux требуются `libgtk-3-dev`, `libwebkit2gtk-4.1-dev` и
графический сеанс X11/XWayland. В Windows требуется установленный WebView2 Runtime.

GPUI WebView пока экспериментален: на используемом backend нативная страница
рисуется поверх GPUI в пределах своей области. Она явно скрывается при смене
вкладки и открытии переменных; размер обновляется вместе с окном. Полная
совместимость с браузерным движком Postman не гарантируется.

Проверка системного WebView без открытия пользовательских запросов:

```bash
cargo run --locked --example visualizer_smoke
```

Тест открывает временное окно, проверяет CSS, canvas, `pm.getData` и изоляцию
страницы и автоматически закрывается. Ожидаемый вывод содержит `"ok":true`.

Семантика API соответствует [описанию Postman Visualizer](https://learning.postman.com/docs/use/send-requests/response-data/visualizer/).
Handlebars 4.7.8 включён в `assets/vendor/handlebars.js`, лицензия MIT — рядом.
