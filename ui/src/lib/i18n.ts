// dsh locale: tiny two-language dictionary (en/ru) + tr()/useT(); no runtime
// deps — "system" resolves once per render through navigator.language
import { useMemo } from "react";
import { useStore } from "@/store/app";

export type Lang = "system" | "en" | "ru";

const LS_KEY = "hiderola.lang";

function loadLang(): Lang {
  const v = localStorage.getItem(LS_KEY);
  return v === "en" || v === "ru" || v === "system" ? v : "system";
}

let current: Lang = loadLang();

// <html lang> follows the resolved language from the very first paint
document.documentElement.lang = resolve(current);

export function getLang(): Lang {
  return current;
}

// persists and applies; components re-render through the store's lang field
export function setLang(l: Lang) {
  current = l;
  localStorage.setItem(LS_KEY, l);
  document.documentElement.lang = resolve(l);
}

export function resolve(l: Lang): "en" | "ru" {
  if (l !== "system") return l;
  return (navigator.language || "").toLowerCase().startsWith("ru") ? "ru" : "en";
}

// [en, ru]; tr() falls back to the key itself so missed spots stay readable
export const DICT: Record<string, [string, string]> = {
  // app shell
  "app.drop_files": ["drop files or folders to attach", "перетащите файлы или папки, чтобы прикрепить"],
  "app.drop_hint": ["they will be added to your next message", "они будут добавлены к следующему сообщению"],
  "app.stop_again": ["press the stop shortcut again to cancel", "нажмите остановку ещё раз, чтобы отменить"],
  "app.attached_n": ["attached {n} item(s)", "прикреплено: {n}"],
  "app.still_running": ["agent is still running", "агент ещё работает"],
  "app.quit_warn": ["quitting now interrupts the current task and its queued messages.", "выход сейчас прервёт текущую задачу и её очередь сообщений."],
  "app.stay": ["stay", "остаться"],
  "app.hide_tray": ["hide to tray", "свернуть в трей"],
  "app.quit": ["quit", "выйти"],

  // generic
  "a.copied": ["copied", "скопировано"],
  "a.cancel": ["cancel", "отмена"],
  "a.close": ["close", "закрыть"],
  "a.delete": ["delete", "удалить"],
  "a.rename": ["rename", "переименовать"],
  "a.save": ["save", "сохранить"],
  "a.copy": ["copy", "копировать"],
  "a.paste": ["paste", "вставить"],
  "a.cut": ["cut", "вырезать"],
  "a.select_all": ["select all", "выбрать всё"],
  "a.open": ["open", "открыть"],
  "a.refresh": ["refresh", "обновить"],
  "a.wait": ["wait for the current reply to finish first", "сначала дождитесь окончания ответа"],
  "a.today": ["today", "сегодня"],
  "a.yesterday": ["yesterday", "вчера"],
  "a.msgs": ["msgs", "сообщ."],
  "a.working_here": ["the agent is working here", "агент работает здесь"],
  "a.reveal": ["show in file manager", "показать в проводнике"],
  "a.new_chat": ["new chat", "новый чат"],

  // sidebar
  "sb.tools": ["tools", "инструменты"],
  "sb.sandbox": ["sandbox", "песочница"],
  "sb.sandbox_t": ["sandbox — local QEMU VMs for isolated agent work", "песочница — локальные QEMU ВМ для изолированной работы агента"],
  "sb.hosts": ["remote hosts", "удалённые хосты"],
  "sb.hosts_t": ["remote hosts — run the agent on your servers over ssh", "удалённые хосты — запуск агента на серверах по ssh"],
  "sb.skills": ["skills", "скиллы"],
  "sb.skills_t": ["skills — plugin folders with SKILL.md, toggles, git install", "скиллы — папки-плагины с SKILL.md, переключатели, git-установка"],
  "sb.crew": ["crew", "команда"],
  "sb.crew_t": ["crew — several AI agents in one session", "команда — несколько ИИ-агентов в одной сессии"],
  "sb.sessions": ["sessions", "сессии"],
  "sb.search": ["search sessions", "поиск по сессиям"],
  "sb.no_history": ["no history yet", "истории пока нет"],
  "sb.no_match": ["no match", "ничего не найдено"],
  "sb.pin": ["pin", "закрепить"],
  "sb.unpin": ["unpin", "открепить"],
  "sb.theme": ["toggle theme", "сменить тему"],
  "sb.settings": ["settings", "настройки"],
  "sb.delete_warn": ["stop the current run before deleting this chat", "остановите текущий запуск перед удалением этого чата"],

  // chat view
  "ch.copy_msg": ["copy message", "копировать сообщение"],
  "ch.copy_sel": ["copy selection", "копировать выделение"],
  "ch.fork_here": ["fork from here", "форкнуть отсюда"],
  "ch.fork_aria": ["fork chat from this message", "форкнуть чат от этого сообщения"],
  "ch.regenerate": ["regenerate", "перегенерировать"],
  "ch.thinking": ["thinking", "размышления"],
  "ch.kill": ["kill", "убить"],
  "ch.expand": ["click to expand", "нажмите, чтобы развернуть"],
  "ch.collapse": ["click to collapse", "нажмите, чтобы свернуть"],
  "ch.jump_latest": ["jump to latest", "к последним сообщениям"],
  "ch.output": ["output", "вывод"],
  "ch.forked_note": ["forked — this chat continues in a new session", "форкнуто — чат продолжается в новой сессии"],
  "ch.queue_restored": ["{n} queued message(s) restored from the previous run; they will steer the next run", "{n} сообщ. из очереди восстановлено; они направят следующий запуск"],
  "ch.exit": ["exit", "код выхода"],

  // input bar
  "in.placeholder": ["ask anything…", "спросите о чём угодно…"],
  "in.send": ["send", "отправить"],
  "in.stop": ["stop", "остановить"],
  "in.attach": ["attach files", "прикрепить файлы"],
  "in.plan": ["plan mode", "режим плана"],
  "in.plan_on": ["plan mode on: read-only research", "режим плана включён: только исследование"],
  "in.plan_off": ["plan mode off", "режим плана выключен"],
  "in.undo": ["undo last change", "отменить последнее изменение"],
  "in.redo": ["redo", "вернуть изменение"],
  "in.queue": ["queue", "очередь"],

  // topbar / palette / hotkeys
  "tb.palette": ["command palette", "палитра команд"],
  "tb.hide": ["hide sidebar", "скрыть панель"],
  "tb.show": ["show sidebar", "показать панель"],
  "pl.placeholder": ["type a command…", "введите команду…"],
  "pl.no_results": ["no matching commands", "команд не найдено"],
  "hk.title": ["keyboard shortcuts", "горячие клавиши"],

  // dialogs
  "dl.confirm_title": ["apply this change?", "применить это изменение?"],
  "dl.ask_title": ["agent needs your input", "агенту нужен ваш ответ"],
  "dl.files_title": ["files", "файлы"],
  "dl.review_title": ["review changes", "ревью изменений"],
  "dl.toast_title": ["notifications", "уведомления"],

  // settings
  "st.title": ["settings", "настройки"],
  "st.provider": ["provider", "провайдер"],
  "st.api_key": ["api key", "api-ключ"],
  "st.model": ["model", "модель"],
  "st.general": ["general", "общее"],
  "st.appearance": ["appearance", "оформление"],
  "st.chat_font": ["chat font size", "размер шрифта чата"],
  "st.language": ["language", "язык"],
  "st.lang_system": ["system", "системный"],
  "st.lang_en": ["english", "english"],
  "st.lang_ru": ["russian", "русский"],
  "st.mcp": ["mcp servers", "mcp-серверы"],
  "st.agent": ["agent", "агент"],
  "st.permissions": ["permissions", "права"],
  "st.saved": ["settings saved", "настройки сохранены"],
  "st.version": ["version", "версия"],

  // sandbox / hosts / skills / crew
  "sx.title": ["sandbox", "песочница"],
  "sx.create": ["create sandbox", "создать песочницу"],
  "sx.attach": ["attach", "подключить"],
  "sx.detach": ["detach", "отключить"],
  "sx.start": ["start", "запустить"],
  "sx.stop": ["stop", "остановить"],
  "sx.hosts_title": ["remote hosts", "удалённые хосты"],
  "sk.enable": ["enable", "включить"],
  "sk.disable": ["disable", "выключить"],
  "cr.goal": ["goal", "цель"],
  "cr.launch": ["launch crew", "запустить команду"],

  // markdown context menus
  "md.open_img": ["open image", "открыть изображение"],
  "md.copy_img": ["copy image address", "копировать адрес изображения"],
  "md.open_link": ["open link", "открыть ссылку"],
  "md.copy_link": ["copy link", "копировать ссылку"],
  "md.reveal": ["reveal value", "показать значение"],

  // slash command ghost hints (mirror src/app.rs HELP)
  "cmd.file": ["attach a file to the next message", "прикрепить файл к следующему сообщению"],
  "cmd.model": ["switch the model (saved to config)", "сменить модель (сохранится в конфиге)"],
  "cmd.models": ["list models for the api key", "список моделей для api-ключа"],
  "cmd.profile": ["switch provider profile", "переключить профиль провайдера"],
  "cmd.plan": ["toggle plan mode (read-only research)", "режим плана (только исследование)"],
  "cmd.undo": ["revert the last turn's file changes", "отменить правки файлов за последний ход"],
  "cmd.redo": ["reapply undone changes", "вернуть отменённые правки"],
  "cmd.init": ["create or improve AGENTS.md", "создать или улучшить AGENTS.md"],
  "cmd.compact": ["summarize and shrink the context", "сжать контекст беседы"],
  "cmd.export": ["save the session as markdown", "сохранить сессию в markdown"],
  "cmd.sessions": ["list saved sessions", "список сохранённых сессий"],
  "cmd.resume": ["switch to a saved session", "перейти к сохранённой сессии"],
  "cmd.mcpauth": ["mcp oauth status / authorize in browser", "mcp oauth: статус / авторизация в браузере"],
  "cmd.mcpres": ["list mcp resources", "список mcp-ресурсов"],
  "cmd.mcpstatus": ["per-server mcp status", "статус mcp-серверов"],
  "cmd.mcpread": ["read an mcp resource into the chat", "прочитать mcp-ресурс в чат"],
  "cmd.mcpadd": ["add an mcp server and connect it", "добавить mcp-сервер и подключить"],
  "cmd.mcpconnect": ["(re)connect a configured server", "(пере)подключить настроенный сервер"],
  "cmd.mcpdisconnect": ["drop the live connection", "разорвать живое соединение"],
  "cmd.mcplog": ["recent mcp log messages", "свежие записи лога mcp"],
  "cmd.jstools": ["list user js tools", "список пользовательских js-инструментов"],
  "cmd.doctor": ["environment self-check", "самопроверка окружения"],
  "cmd.update": ["check for updates and install", "проверить обновления и установить"],
  "cmd.usage": ["token usage and context left", "расход токенов и остаток контекста"],
  "cmd.sandbox": ["open the sandbox panel", "открыть панель песочниц"],
  "cmd.host": ["open the remote hosts panel", "открыть панель удалённых хостов"],
  "cmd.skills": ["open the skills panel", "открыть панель скиллов"],
  "cmd.crew": ["open the crew panel", "открыть панель команды"],
  "cmd.clear": ["clear this conversation", "очистить беседу"],
  "cmd.quit": ["quit hi-derola", "выйти из hi-derola"],
  "cmd.help": ["list the commands", "список команд"],

  // permission toggles
  "perm.allow": ["allow", "разрешить"],
  "perm.ask": ["ask", "спрашивать"],
  "perm.deny": ["deny", "запретить"],

  // more generic
  "a.clear": ["clear", "очистить"],
  "a.copy_path": ["copy path", "копировать путь"],
  "a.remove": ["remove", "убрать"],

  // ask dialog
  "ak.title": ["agent question", "вопрос агента"],
  "ak.pick": ["pick an option", "выберите вариант"],
  "ak.none_opt": ["no options", "нет вариантов"],
  "ak.own_answer": ["or write your own…", "или напишите свой…"],
  "ak.skip": ["skip", "пропустить"],
  "ak.answer": ["answer", "ответить"],
  "ak.value": ["value…", "значение…"],

  // confirm (tool run) dialog
  "cf.run": ["run {name}?", "запустить {name}?"],
  "cf.run_btn": ["run", "запустить"],
  "cf.allow_all": ["allow all", "разрешить всё"],
  "cf.always": ["always allow", "всегда разрешать"],
  "cf.reject": ["reject", "отклонить"],
  "cf.feedback": ["message to the agent (optional)…", "сообщение агенту (необязательно)…"],

  // files dialog / composer chips
  "fl.attach": ["attach", "прикрепить"],
  "fl.attach_this": ["attach this folder", "прикрепить эту папку"],
  "fl.empty": ["this folder is empty", "эта папка пуста"],
  "fl.filter": ["filter files…", "фильтр файлов…"],
  "fl.hint": ["click a file to attach it", "нажмите на файл, чтобы прикрепить"],
  "fl.no_match": ["nothing matches “{q}”", "по «{q}» ничего нет"],
  "fl.parent": ["up one level", "на уровень выше"],
  "fl.this_folder": ["use this folder", "взять эту папку"],
  "fm.finder": ["show in finder", "показать в finder"],
  "fm.explorer": ["show in explorer", "показать в проводнике"],
  "fm.folder": ["show in folder", "показать в папке"],

  // hotkeys dialog + hotkey actions
  "hk.search": ["search shortcuts", "поиск по сочетаниям"],
  "hk.clear_search": ["clear search", "очистить поиск"],
  "hk.no_match": ["no shortcuts match", "ничего не найдено"],
  "hk.customize": ["customize", "настроить"],
  "hk.bindings": ["{n} bound", "назначено: {n}"],
  "hk.send": ["send message", "отправить сообщение"],
  "hk.newline": ["insert newline", "перевод строки"],
  "hk.stop": ["stop generation", "остановить генерацию"],
  "hk.new_session": ["new session", "новая сессия"],
  "hk.open_settings": ["open settings", "открыть настройки"],
  "hk.toggle_sidebar": ["toggle sidebar", "показать/скрыть панель"],
  "hk.toggle_theme": ["toggle theme", "сменить тему"],
  "hk.toggle_plan": ["toggle plan mode", "режим плана"],
  "hk.undo": ["undo file changes", "отменить правки файлов"],
  "hk.redo": ["redo file changes", "вернуть правки файлов"],
  "hk.toggle_thinking": ["toggle thinking", "показать/скрыть размышления"],
  "hk.palette": ["command palette", "палитра команд"],
  "hk.shortcuts": ["shortcut reference", "справочник сочетаний"],
  "hk.menu_move": ["move in menus", "навигация в меню"],
  "hk.menu_select": ["select in menus", "выбор в меню"],
  "hk.menu_dismiss": ["dismiss menus", "закрыть меню"],
  "hk.g_app": ["application", "приложение"],
  "hk.g_input": ["message input", "ввод сообщений"],
  "hk.g_menus": ["shared menu keys", "общие клавиши меню"],

  // remote hosts
  "hs.sub": ["run the agent on your servers over ssh", "запускайте агента на своих серверах по ssh"],
  "hs.empty": ["no hosts yet — add one above", "хостов пока нет — добавьте выше"],
  "hs.add": ["add host", "добавить хост"],
  "hs.user": ["user", "пользователь"],
  "hs.host": ["host", "хост"],
  "hs.port": ["port", "порт"],
  "hs.save_gen": ["save & install key", "сохранить и установить ключ"],
  "hs.key_hint": ["on save a keypair is generated and the public key is installed on the host", "при сохранении создастся пара ключей, публичный установится на хосте"],
  "hs.saved_key": ["host {name} saved — public key: {key}", "хост {name} сохранён — публичный ключ: {key}"],
  "hs.add_key": ["public key installed on {label}: {key}", "публичный ключ установлен на {label}: {key}"],
  "hs.agent_installed": ["agent installed", "агент установлен"],
  "hs.check": ["check", "проверить"],
  "hs.check_failed": ["check failed: {err}", "проверка не прошла: {err}"],
  "hs.reachable": ["{name} is reachable", "{name} доступен"],
  "hs.attach_t": ["run bash and file tools on this host", "запускать bash и файловые инструменты на этом хосте"],
  "hs.probe_t": ["probe ssh connectivity", "проверить ssh-соединение"],
  "hs.pubkey_t": ["(re)install the public key", "(пере)установить публичный ключ"],

  // input bar extras
  "in.agent_general": ["general agent", "общий агент"],
  "in.agent_explore": ["explore agent", "агент-исследователь"],
  "in.attach_hint": ["attach files or folders", "прикрепить файлы или папки"],
  "in.browse": ["browse", "обзор"],
  "in.choose_files": ["choose files…", "выбрать файлы…"],
  "in.choose_folder": ["choose folder…", "выбрать папку…"],
  "in.or_drop": ["or drop them anywhere", "или просто перетащите сюда"],
  "in.folder_attached": ["folder attached", "папка прикреплена"],
  "in.commands": ["commands", "команды"],
  "in.ctx": ["context", "контекста"],
  "in.ctx_title": ["context: {used} of {limit} tokens", "контекст: {used} из {limit} токенов"],
  "in.edit": ["edit this message", "редактировать сообщение"],
  "in.placeholder_full": ["ask anything — / for commands", "спросите о чём угодно — / для команд"],
  "in.plan_hint": ["plan mode: read-only research", "режим плана: только исследование"],
  "in.queue_after": ["queue after the current run", "в очередь после текущего запуска"],
  "in.run_next": ["run this queued message now", "запустить это сообщение сейчас"],
  "in.runs_after": ["runs after the current reply finishes", "выполнится после текущего ответа"],
  "in.starting": ["starting the agent", "запуск агента"],
  "in.todo_active": ["{n} active", "в работе: {n}"],
  "in.todo_done": ["{n} done", "готово: {n}"],
  "in.todo_pending": ["{n} pending", "ожидают: {n}"],
  "in.todo_dropped": ["{n} dropped", "отменено: {n}"],
  "in.todos": ["todos", "задачи"],
  "in.tok_in": ["in", "вход"],
  "in.tok_out": ["out", "выход"],
  "in.tok_cached": ["cached", "кэш"],
  "in.working": ["working", "работаю"],

  // lightbox
  "lb.close": ["close", "закрыть"],
  "lb.title": ["image preview", "просмотр изображения"],

  // model menu
  "mm.load": ["load models", "загрузить модели"],
  "mm.loading": ["loading…", "загрузка…"],
  "mm.refresh": ["refresh", "обновить"],
  "mm.no_models": ["no models available for this api key", "для этого api-ключа нет моделей"],
  "mm.search": ["search models…", "поиск моделей…"],
  "mm.switch": ["switch the model", "сменить модель"],
  "mm.switched": ["model switched to {name}", "модель переключена на {name}"],

  // palette
  "pl.nav_hint": ["↑↓ to navigate · enter to run · esc to close", "↑↓ навигация · enter запуск · esc закрыть"],
  "pl.ref_h": ["full shortcut reference", "полный справочник сочетаний"],
  "pl.review": ["review changes", "ревью изменений"],
  "pl.review_h": ["diff of the current run", "диф текущего запуска"],
  "pl.sandbox": ["sandbox", "песочница"],
  "pl.sandbox_h": ["local qemu vms", "локальные qemu-вм"],
  "pl.settings_h": ["open settings", "открыть настройки"],
  "pl.sidebar_h": ["show or hide the sidebar", "показать или скрыть панель"],

  // review dialog
  "rv.title": ["review changes", "ревью изменений"],
  "rv.empty": ["no changes in this run yet", "в этом запуске изменений пока нет"],
  "rv.counts_only": ["counts only", "только счётчики"],
  "rv.footer": ["{n} file(s), +{adds} −{dels}", "файлов: {n}, +{adds} −{dels}"],
  "rv.patches": ["patch: {n} file(s), +{adds} −{dels}", "патч: файлов {n}, +{adds} −{dels}"],
  "rv.note": ["read-only — the edits are already applied", "только просмотр — правки уже применены"],

  // skills
  "sk.sub": ["put skill folders into", "положите папки скиллов в"],
  "sk.sub2": ["or", "или"],
  "sk.sub3": ["then rescan", "затем пересканируйте"],
  "sk.rescan": ["rescan", "пересканировать"],
  "sk.git_install": ["install from git", "установка из git"],
  "sk.clone": ["clone", "клонировать"],
  "sk.clone_hint": ["any public repo with skills", "любой публичный репозиторий со скиллами"],
  "sk.cloning": ["cloning…", "клонирую…"],
  "sk.empty": ["no skills yet", "скиллов пока нет"],

  // settings extras
  "st.chat": ["chat", "чат"],
  "st.stream": ["streaming", "стриминг"],
  "st.theme_hint": ["accent color and dark / light mode", "акцентный цвет и тёмная / светлая тема"],
  "st.save_hint": ["changes apply and save immediately", "изменения применяются и сохраняются сразу"],
  "st.font_dec": ["decrease chat font", "уменьшить шрифт чата"],
  "st.font_inc": ["increase chat font", "увеличить шрифт чата"],
  "st.font_hint": ["applies to chat messages only (12–18px)", "влияет только на сообщения чата (12–18px)"],
  "st.reset": ["reset", "сброс"],
  "st.lang_hint": ["applies instantly; “system” follows the os language", "применяется сразу; «системный» следует языку ос"],
  "st.base_url": ["base url", "базовый url"],
  "st.show_key": ["show the api key", "показать api-ключ"],
  "st.hide_key": ["hide the api key", "скрыть api-ключ"],
  "st.max_tokens": ["max tokens", "макс. токенов"],
  "st.temperature": ["temperature", "температура"],
  "st.load_models": ["load models", "загрузить модели"],
  "st.load_models_hint": ["fetch the model list for this api key", "получить список моделей для этого api-ключа"],
  "st.loading_models": ["loading models…", "загружаю модели…"],
  "st.models_loaded": ["{n} models loaded", "загружено моделей: {n}"],
  "st.model_required": ["enter a model name", "укажите модель"],
  "st.save_failed": ["could not save — check the fields", "не сохранилось — проверьте поля"],
  "st.url_required": ["base url is required", "нужен базовый url"],
  "st.name_required": ["name is required", "нужно имя"],
  "st.name_exists": ["a server with this name already exists", "сервер с таким именем уже есть"],
  "st.server_added": ["server added", "сервер добавлен"],
  "st.cmd_required": ["command is required for a stdio server", "для stdio-сервера нужна команда"],
  "st.mcp_empty": ["no mcp servers yet — add one below", "mcp-серверов пока нет — добавьте ниже"],
  "st.add": ["add", "добавить"],
  "st.add_server": ["add server", "добавить сервер"],
  "st.reconnect": ["reconnect", "переподключить"],
  "st.run": ["run", "запустить"],
  "st.m_name": ["name", "имя"],
  "st.m_type": ["type", "тип"],
  "st.m_url": ["url", "url"],
  "st.m_command": ["command", "команда"],
  "st.m_args": ["args", "аргументы"],
  "st.m_args_ph": ["e.g. -y @modelcontextprotocol/server-fs /tmp", "например -y @modelcontextprotocol/server-fs /tmp"],
  "st.m_env": ["env", "переменные"],
  "st.m_env_ph": ["KEY=value, one per line", "КЛЮЧ=значение, по строке"],
  "st.m_headers": ["headers", "заголовки"],
  "st.m_headers_ph": ["Authorization: Bearer …", "Authorization: Bearer …"],
  "st.m_cwd": ["working dir", "рабочая папка"],
  "st.m_optional": ["optional", "необязательно"],
  "st.m_timeout": ["timeout, s", "таймаут, с"],
  "st.m_default": ["default", "по умолчанию"],
  "st.m_remove_t": ["remove this server", "удалить этот сервер"],
  "st.m_res_t": ["browse resources", "ресурсы сервера"],
  "st.m_resources": ["{n} resource(s)", "ресурсов: {n}"],
  "st.m_prompts": ["{n} prompt(s)", "промптов: {n}"],
  "st.m_templates": ["{n} uri templates", "шаблонов uri: {n}"],
  "st.m_too_big": ["“{uri}” is {n} chars — too big for the composer", "«{uri}» слишком велик для ввода: {n} символов"],
  "st.m_sub_t": ["toggle the update subscription", "переключить подписку на обновления"],
  "st.m_saved": ["saved to the config", "сохранится в конфиге"],
  "st.none": ["none", "нет"],
  "st.press_keys": ["press keys…", "нажмите сочетание…"],
  "st.click_record": ["click to record a new shortcut", "нажмите, чтобы записать сочетание"],
  "st.reset_default": ["reset to default", "вернуть по умолчанию"],
  "st.hotkeys": ["hotkeys", "горячие клавиши"],
  "st.hotkeys_hint": ["overrides are stored in the config", "переопределения хранятся в конфиге"],
  "st.open_ref": ["open the reference", "открыть справочник"],
  "st.p_edit": ["edit files", "правка файлов"],
  "st.p_write": ["write files", "запись файлов"],
  "st.p_bash": ["shell commands", "команды оболочки"],
  "st.p_sandbox": ["sandbox vms", "песочницы"],
  "st.p_mcp": ["mcp tools", "mcp-инструменты"],
  "st.p_webfetch": ["fetching urls", "загрузка url"],
  "st.p_websearch": ["web search", "веб-поиск"],
  "st.p_codesearch": ["code search", "поиск по коду"],
  "st.p_subagent": ["subagents", "суб-агенты"],
  "st.perm_full": ["full access", "полный доступ"],
  "st.perm_full_d": ["every tool runs without asking", "любой инструмент выполняется без вопросов"],
  "st.perm_ask": ["ask first", "спрашивать"],
  "st.perm_ask_d": ["confirm every tool run", "подтверждать каждый запуск инструмента"],
  "st.perm_custom": ["custom", "настройка"],
  "st.perm_custom_d": ["allow / ask / deny per tool", "разрешить / спрашивать / запретить для каждого"],
  "st.perm_rules_n": ["{n} custom rule(s) below", "своих правил ниже: {n}"],
  "st.perm_rules": ["no custom rules — the defaults apply", "своих правил нет — работают базовые"],
  "st.perm_any_mode": ["this mode applies to every tool", "режим действует на все инструменты"],
  "st.profile_active": ["profile “{name}” is active", "активен профиль «{name}»"],
  "st.click_ver": ["click to copy the version", "нажмите, чтобы скопировать версию"],
  "st.ver_copied": ["copied {v}", "скопировано: {v}"],
  "st.desktop": ["desktop", "десктоп"],
  "st.github": ["github", "github"],
  "st.about": ["about", "о программе"],
  "st.risk_title": ["enable full access?", "включить полный доступ?"],
  "st.risk_body": ["the agent will run every tool without confirmation — file writes, shell commands, mcp calls", "агент будет выполнять любой инструмент без подтверждения — запись файлов, команды оболочки, вызовы mcp"],
  "st.risk_ack": ["i understand the risks", "я понимаю риски"],
  "st.risk_enable": ["enable", "включить"],

  // sandbox panel extras
  "sx.agent_not_installed": ["agent is not installed in this vm", "агент в этой вм не установлен"],
  "sx.needs_root": [" (a root image is required)", " (нужен root-образ)"],
  "sx.agent_ready": ["agent ready{v}", "агент готов{v}"],
  "sx.installing_agent": ["installing agent…", "ставлю агента…"],
  "sx.install_failed": ["agent install failed", "не удалось поставить агента"],
  "sx.attach_t": ["attach the chat session to this vm — bash and file tools run inside", "привязать сессию чата к этой вм — bash и файлы будут работать внутри"],
  "sx.detach_t": ["detach the session from this vm", "отвязать сессию от этой вм"],
  "sx.bash_here": ["bash here", "bash здесь"],
  "sx.bash_here_t": ["this session's bash and file tools run in this vm", "bash и файловые инструменты сессии работают в этой вм"],
  "sx.terminal": ["terminal", "терминал"],
  "sx.terminal_t": ["open a shell in the vm", "открыть оболочку в вм"],
  "sx.run_agent": ["run agent", "запустить агента"],
  "sx.run_agent_t": ["open an agent terminal in the vm", "открыть терминал агента в вм"],
  "sx.build_agent_t": ["build and install the agent inside the vm", "собрать и установить агента внутри вм"],
  "sx.install_agent": ["install agent", "установить агента"],
  "sx.reinstall_agent": ["reinstall agent", "переустановить агента"],
  "sx.cancel_delete": ["cancel the download", "отменить загрузку"],
  "sx.delete_sbx": ["delete this sandbox", "удалить эту песочницу"],
  "sx.disk": ["disk", "диск"],
  "sx.ram": ["ram", "озу"],
  "sx.downloading": ["downloading", "загрузка"],
  "sx.fetch": ["fetch a file", "вытянуть файл"],
  "sx.fetch_t": ["copy a file from the vm to the host", "скопировать файл из вм на хост"],
  "sx.fetch_prompt": ["path in {name}:", "путь в {name}:"],
  "sx.fetching": ["fetching…", "копирую…"],
  "sx.saved_to": ["saved to {path}", "сохранено в {path}"],
  "sx.first_boot": ["first boot downloads the image", "при первом запуске скачается образ"],
  "sx.fwd_add": ["forward a port", "пробросить порт"],
  "sx.fwd_del_t": ["click to remove this forward", "нажмите, чтобы убрать проброс"],
  "sx.fwd_need": ["use the host:guest format, e.g. 8080:80", "используйте формат хост:гость, например 8080:80"],
  "sx.fwd_prompt": ["host:guest, e.g. 8080:80", "хост:гость, например 8080:80"],
  "sx.root_ok": ["root image", "root-образ"],
  "sx.no_root": ["rootless image", "без root"],
  "sx.reboot_hint": ["restart the vm to apply", "перезапустите вм, чтобы применить"],
  "sx.retry": ["retry", "повторить"],
  "sx.up_in": ["up for {n}s", "работает {n} с"],

  // topbar
  "tb.copy_md": ["copy the session as markdown", "скопировать сессию в markdown"],
  "tb.md_ok": ["session copied as markdown", "сессия скопирована в markdown"],
  "tb.md_fail": ["could not copy the session", "не удалось скопировать сессию"],
  "tb.review_hint": ["review: {n} file(s), +{adds} −{dels}", "ревью: файлов {n}, +{adds} −{dels}"],
};

export function tr(key: string, vars?: Record<string, string | number>): string {
  const pair = DICT[key];
  let s = pair ? (resolve(current) === "ru" ? pair[1] : pair[0]) : key;
  if (vars) {
    for (const [k, v] of Object.entries(vars)) s = s.replaceAll(`{${k}}`, String(v));
  }
  return s;
}

// reactive translate for components: re-created whenever the language changes
export function useT() {
  const lang = useStore((s) => s.lang);
  return useMemo(() => {
    const l = resolve(lang);
    return (key: string, vars?: Record<string, string | number>) => {
      const pair = DICT[key];
      let s = pair ? (l === "ru" ? pair[1] : pair[0]) : key;
      if (vars) {
        for (const [k, v] of Object.entries(vars)) s = s.replaceAll(`{${k}}`, String(v));
      }
      return s;
    };
  }, [lang]);
}
