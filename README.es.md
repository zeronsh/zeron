# Zeron

Controla tus agentes de programación (Claude Code, Codex, Cursor, Devin, Grok, Hermes, Pi, Antigravity) de forma local por defecto, con sincronización opcional entre varios dispositivos.

*[English](README.md) | [简体中文](README.zh-CN.md) | [한국어](README.ko.md) | [日本語](README.ja.md) | Español*

![App de escritorio de Zeron](docs/media/readme/app-screenshot.jpg)

## App de escritorio

Descarga la última versión para tu plataforma desde [GitHub Releases](https://github.com/zeronsh/zeron/releases/latest):

- **macOS** — `zeron-<version>-macos-arm64.dmg`
- **Windows** — `zeron-<version>-windows-x86_64-setup.exe`
- **Linux** — `zeron-<version>-linux-<arch>.tar.gz` y luego ejecuta el `install.sh` que incluye

No necesitas cuenta ni conexión a internet; las sesiones se quedan en tu dispositivo. La app se actualiza sola.

## Modo headless (CLI)

Para servidores y otras máquinas sin pantalla, como un VPS que mantiene los agentes en ejecución después de cerrar tu portátil. Solo Linux:

```bash
curl -fsSL https://zeron.sh/install.sh | sh
zeron status
```

El instalador arranca el motor como un servicio en segundo plano que sobrevive a los reinicios.

```bash
zeron status      # modo local/sincronizado y estado del motor
zeron update      # actualizar a la última versión
zeron daemon start|stop|restart|status
```

## Sincronización entre dispositivos (opcional)

Inicia sesión para lanzar un agente en un dispositivo y seguirlo o controlarlo desde otro:

```bash
zeron daemon stop
zeron login        # o: zeron logout para volver al modo solo local
zeron daemon start
```

Los dispositivos con sesión iniciada en la misma cuenta pueden leer y escribir los archivos del espacio de trabajo de los demás, así que inicia sesión solo en dispositivos de confianza. Las sesiones locales existentes nunca se suben.

## Patrocinadores

Gracias a [The Context Company](https://www.thecontextcompany.com/) por patrocinar Zeron. Tú también puedes ayudar a financiar el desarrollo de Zeron [convirtiéndote en patrocinador en GitHub](https://github.com/sponsors/zeronsh).

---

¿Quieres contribuir o te interesa saber cómo funciona? [Pregúntale a DeepWiki](https://deepwiki.com/zeronsh/zeron) o consulta [ARCHITECTURE.md](ARCHITECTURE.md).

Publicado bajo la [licencia MIT](LICENSE).
