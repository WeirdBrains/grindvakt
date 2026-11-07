import 'package:jaspr/jaspr.dart';
import 'package:jaspr/dom.dart';

class GrindvaktSite extends StatelessComponent {
  const GrindvaktSite({super.key});

  @override
  Component build(BuildContext context) {
    return div(classes: 'page', [
      // Load external CSS built with the site.
      link(href: '/styles.css', attributes: {'rel': 'stylesheet'}),
      header(classes: 'site-header', [
        a(href: '#', classes: 'brand', [
          img(
            src: '/assets/grindvakt-guard-logo.jpg',
            attributes: {'alt': 'Grindvakt guard logo'},
          ),
          .text('Grindvakt'),
        ]),
        nav(classes: 'nav', [
          a(href: '#install', [.text('Install')]),
          a(href: '#setup', [.text('Setup')]),
          a(href: '#security', [.text('Security')]),
          a(href: 'https://github.com/WeirdBrains/grindvakt/releases/latest', [
            .text('Releases'),
          ]),
        ]),
      ]),
      main_([
        section(id: 'hero', classes: 'hero', [
          div(classes: 'hero-inner', [
            div(classes: 'hero-left', [
              h1([.text('Your vigilant DNS guard')]),
              p([
                .text(
                  'Grindvakt keeps your A record updated to your real public IPv4, '
                  'secured with an encrypted vault and a 24‑word recovery key. '
                  'Built in Rust. systemd —user, no root daemons.',
                ),
              ]),
              div(classes: 'cta', [
                a(
                  href: 'https://github.com/WeirdBrains/grindvakt/releases/latest',
                  classes: 'btn primary',
                  [.text('Download installers')],
                ),
                a(href: '#install', classes: 'btn', [
                  .text('Installation guide'),
                ]),
              ]),
            ]),
            div(classes: 'hero-right', [
              // Use the existing guard logo prominently as the hero ornament.
              img(
                src: '/assets/grindvakt-guard-logo.jpg',
                classes: 'mjolnir',
                attributes: {'alt': '', 'aria-hidden': 'true'},
              ),
            ]),
          ]),
        ]),
        section(id: 'features', classes: 'features', [
          _feature(
            'Majority IP consensus',
            'Counts multiple sources and only accepts a strict majority. '
                'No ties. No first‑two shortcuts.',
          ),
          _feature(
            'Encrypted vault + recovery',
            'XChaCha20‑Poly1305 DEK, Argon2id password wrap, 24‑word recovery. '
                'Zero secrets on argv/env/logs.',
          ),
          _feature(
            'DNSimple integration',
            'Create/update/no‑op A records with the official SDK. Pagination and idempotency by default.',
          ),
          _feature(
            'systemd —user',
            'Oneshot service + timer, explicit enrollment. No root, no auto‑enable.',
          ),
        ]),
        section(id: 'install', classes: 'install', [
          h2([.text('Install')]),
          p([
            .text(
              'Download an installer or the portable tarball from the latest private GitHub release (access required).',
            ),
          ]),
          ul(classes: 'downloads', [
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/latest',
                [.text('Linux x86_64 (musl, tar.gz)')],
              ),
            ]),
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/latest',
                [.text('Debian/Ubuntu (.deb)')],
              ),
            ]),
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/latest',
                [.text('Fedora/RHEL (.rpm) — unsigned (use --nosignature)')],
              ),
            ]),
            li([
              a(
                href:
                    'https://github.com/WeirdBrains/grindvakt/releases/latest',
                [.text('Arch Linux (.pkg.tar.zst)')],
              ),
            ]),
          ]),
        ]),
        section(id: 'setup', classes: 'setup', [
          h2([.text('Setup (systemd —user)')]),
          ol([
            li([
              code([.text('grindvakt init')]),
            ]),
            li([
              code([.text('grindvakt enroll --tpm=auto')]),
            ]),
            li([
              code([.text('grindvakt service install')]),
            ]),
            li([
              code([.text('systemctl --user enable --now grindvakt.timer')]),
            ]),
          ]),
          p([
            .text(
              'Enrollment is explicit; no unattended prompts on non‑TTY. TPM fallback is not automatic.',
            ),
          ]),
        ]),
        section(id: 'security', classes: 'security', [
          h2([.text('Security notes')]),
          ul([
            li([
              .text(
                'Strict majority for public IP; refuses ties with < 2 sources.',
              ),
            ]),
            li([.text('Vault file permissions validated; symlinks rejected.')]),
            li([
              .text(
                'Dry‑run never writes DNS. Tests cover pagination, duplicates, errors.',
              ),
            ]),
          ]),
        ]),
      ]),
      footer(classes: 'site-footer', [
        .text('© '),
        .text('${DateTime.now().year} '),
        a(href: 'https://github.com/WeirdBrains/grindvakt', [
          .text('WeirdBrains / Grindvakt'),
        ]),
      ]),
    ]);
  }

  Component _feature(String title, String body) {
    return div(classes: 'feature', [
      h3([.text(title)]),
      p([.text(body)]),
    ]);
  }
}
