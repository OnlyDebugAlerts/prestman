# Prestman на своём сервере

Установите Docker с Compose и Python 3. Команды выполняются из корня репозитория на сервере. Compose запускает PostgreSQL, Supabase Auth, PostgREST и шлюз Nginx. База хранится в томе Docker.

## Сервер с HTTPS

1. Направьте DNS-запись домена, например `cloud.example.com`, на VPS и откройте входящие TCP-порты 80 и 443.
2. Запустите бэкенд и создайте первую учётную запись.

   ```sh
   python3 self-host/manage.py up --domain cloud.example.com
   python3 self-host/manage.py create-user alice@example.com
   ```

3. Проверьте доступность шлюза по адресу `https://cloud.example.com/health`. HTTP 204 означает, что DNS, сертификат и шлюз работают. Caddy получает и обновляет сертификат автоматически. [Настройка HTTPS в Caddy](https://caddyserver.com/docs/automatic-https)

Команда `up` один раз генерирует серверные ключи в `self-host/.env` и применяет миграции. При повторном запуске она использует тот же том и сохранённые настройки. Параметр `--domain` можно добавить и к уже работающему локальному серверу.

## Локальный сервер или доступ через SSH

```sh
python3 self-host/manage.py up
python3 self-host/manage.py create-user alice@example.com
```

Шлюз слушает `http://127.0.0.1:8800`. Если порт занят, задайте другой через `up --port 8801`. При подключении с другого компьютера откройте SSH-туннель и оставьте его открытым на время работы.

```sh
ssh -N -o ExitOnForwardFailure=yes -L 127.0.0.1:8800:127.0.0.1:8800 user@your-server
```

## Подключение участников

1. Создайте подтверждённую учётную запись для каждого участника командой `create-user`. Она запрашивает пароль интерактивно. Передайте начальный пароль по защищённому каналу. Публичная регистрация отключена, SMTP для этого сценария не требуется.
2. Передайте участникам **Server URL** из вывода `up`. Чтобы снова его посмотреть, выполните `python3 self-host/manage.py client-info`.
3. В Prestman выберите **Connect to Cloud**, заполните **Server URL**, нажмите **Save server** и войдите. Чтобы изменить адрес, нажмите **Sign out**, затем **Change server** на экране входа.
4. Создайте пространство через **New workspace**. Владелец приглашает коллегу по email. Коллега вводит код через **Manage workspaces → Join with code**.

Для запуска клиента из терминала `python3 self-host/manage.py client-env` выводит `PRESTMAN_CLOUD_URL`.

На существующем сервере после обновления файлов повторите `python3 self-host/manage.py up` и используйте обновлённый клиент. Compose пересоздаст шлюз с новой конфигурацией. Если участник подключается к старому шлюзу, в форме подключения есть **Add public API key**.

## Обслуживание

`python3 self-host/manage.py status` показывает состояние контейнеров. `python3 self-host/manage.py down` останавливает сервер, сохраняя том базы. Сохраните дамп и серверный файл `self-host/.env` в защищённом хранилище.

```sh
umask 077
docker compose --env-file self-host/.env -f self-host/compose.yml exec -T db \
  pg_dump -U postgres -d prestman -Fc > prestman-backup.dump
```

Для закрытой сети можно заранее перенести образы через `docker save` и `docker load`.
