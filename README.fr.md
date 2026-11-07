# Zeron

Pilotez vos agents de programmation (Claude Code, Codex, Cursor, Devin, Grok, Hermes, Pi, Antigravity) en local par défaut, avec une synchronisation facultative entre plusieurs appareils.

*[English](README.md) | [简体中文](README.zh-CN.md) | [한국어](README.ko.md) | [日本語](README.ja.md) | [Español](README.es.md) | Français*

![Application de bureau Zeron](docs/media/readme/app-screenshot.jpg)

## Application de bureau

Téléchargez la dernière version pour votre plateforme depuis [GitHub Releases](https://github.com/zeronsh/zeron/releases/latest) :

- **macOS** — `zeron-<version>-macos-arm64.dmg`
- **Windows** — `zeron-<version>-windows-x86_64-setup.exe`
- **Linux** — `zeron-<version>-linux-<arch>.tar.gz`, puis lancez le `install.sh` qu'il contient

Aucun compte ni connexion réseau n'est nécessaire ; les sessions restent sur votre appareil. L'application se met à jour toute seule.

## Mode headless (CLI)

Pour les serveurs et autres machines sans écran, comme un VPS qui garde les agents en marche une fois votre portable refermé. Linux uniquement :

```bash
curl -fsSL https://zeron.sh/install.sh | sh
zeron status
```

L'installateur démarre le moteur comme un service en arrière-plan qui survit aux redémarrages.

```bash
zeron status      # mode local/synchronisé et état du moteur
zeron update      # mettre à jour vers la dernière version
zeron daemon start|stop|restart|status
```

## Synchronisation entre appareils (facultative)

Connectez-vous pour lancer un agent sur un appareil et le suivre ou le piloter depuis un autre :

```bash
zeron daemon stop
zeron login        # ou : zeron logout pour revenir au mode local uniquement
zeron daemon start
```

Les appareils connectés au même compte peuvent lire et écrire les fichiers de l'espace de travail les uns des autres : ne connectez donc que des appareils de confiance. Les sessions locales existantes ne sont jamais envoyées.

## Sponsors

Merci à [The Context Company](https://www.thecontextcompany.com/) de sponsoriser Zeron. Vous aussi pouvez financer le développement de Zeron en [devenant sponsor sur GitHub](https://github.com/sponsors/zeronsh).

---

Vous voulez contribuer ou comprendre comment ça marche ? [Demandez à DeepWiki](https://deepwiki.com/zeronsh/zeron) ou consultez [ARCHITECTURE.md](ARCHITECTURE.md).

Distribué sous [licence MIT](LICENSE).
