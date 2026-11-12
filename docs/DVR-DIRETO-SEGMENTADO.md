# DVR direto segmentado — SaimoWin (Windows / Linux / macOS)

O gravador foi reorganizado para **copiar a transmissão de áudio e vídeo**
com FFmpeg. Ele **não utiliza OBS nem captura da tela**. Não é necessário
instalar codecs ou drivers de captura: o distribuído ao usuário é um binário
FFmpeg portátil compatível com o sistema operacional.

## Fluxo de gravação

1. O usuário escolhe Gravar (até Parar ou por duração) ou Agenda.
2. O Saimo usa a fonte selecionada do canal, com user-agent/referer quando
   publicados na lista. Não tenta contornar criptografia ou DRM.
3. O FFmpeg abre a transmissão e usa `-c copy`, sem reencodificação.
4. A sessão grava `parte_00000.mkv`, `parte_00001.mkv`, ... em um diretório
   exclusivo, com trechos aproximados de 30 s **dependentes de keyframes**.
5. A interface só confirma gravação após verificar bytes efetivamente
   escritos; caso falhe, exibe o caminho do log local.
6. Ao encerrar, tenta unir os trechos em `gravacao_final.mkv` por cópia de
   pacotes. Os arquivos `parte_*.mkv` são mantidos para recuperação.
7. O `ffprobe` verifica as faixas do arquivo final quando está disponível.

Uma sessão contém:

```
Gravacoes/
  Nome-do-canal_2026-11-12_12-30-00/
    sessao.json
    tentativa_1.ffmpeg.log
    tentativa_2.ffmpeg.log (somente se necessário)
    parte_00000.mkv
    parte_00001.mkv
    partes.ffconcat
    gravacao_final.mkv (após união bem-sucedida)
```

## Particularidades de fontes HLS

O mpv original já força `demuxer-lavf-format=hls` para URL terminada em
`.txt`; o DVR agora usa equivalente `-f hls` nesse caso. Na segunda
tentativa, se a primeira falhou antes de gravar qualquer byte, inclui uma
lista explícita de extensões conhecidas com a extensão atípica `.woff`
para listas HLS. Não utiliza `ALL` e não desabilita globalmente a
verificação de formatos.

Esse mecanismo não garante que URL assinada/expirada possa ser gravada:
a aplicação ainda precisa receber a transmissão legitimamente. Se o
servidor responder `error.txt`, o DVR registrará falha real, sem inventar
arquivo reproduzível.

## Segurança e recuperação

- Arquivos de sessões não são sobrescritos nem apagados ao juntar.
- O manifesto `sessao.json` é gravado em arquivo temporário e renomeado.
- O processo tenta parar de forma limpa via `q`; se não responder, preserva
  os trechos anteriores e encerra o processo.
- No próximo início do aplicativo, identifica sessões interrompidas e
  tenta remontá-las **após estabilização** do tamanho dos arquivos.
- Falta de energia, falha de disco ou encerramento durante escrita de
  um segmento podem causar perda do segmento incompleto; não há garantia
  matemática de preservação dos últimos quadros.
- Logs locais do FFmpeg podem conter URLs e tokens. Não publique esses logs
  sem remover dados de acesso.

## Configuração por dispositivo

- Windows: pasta de Vídeos do usuário (ou destino escolhido em Ajustes).
- Linux: pasta de vídeos do usuário / XDG.
- macOS: Movies do usuário.
- Para teste: variável `SAIMO_GRAVACOES_DIR` com um caminho absoluto.
- Deve acompanhar o aplicativo um FFmpeg e, para validação, um ffprobe
  compatíveis com a plataforma. Nenhum OBS é necessário.

## Agendamentos

O agendador atual funciona enquanto o Saimo permanece aberto num
computador ligado. A gravação não exige manter o canal no primeiro plano,
pois é independente do player. Iniciar com o Saimo fechado ainda requer
um agente ou integração futura com o Agendador de Tarefas / systemd.

## Testes

- Testar canal TV aberta que já falhava com `.txt` / `.woff`.
- Testar fonte HLS normal `.m3u8`.
- Confirmar segmentos independentes após mais de 35 s.
- Confirmar vídeo + áudio no `ffprobe` e reprodução de `gravacao_final.mkv`.
- Encerrar processo Saimo abruptamente: trechos anteriores devem permanecer.
- Deixar Saimo aberto e agendar um canal em segundo plano.
- Verificar comportamento em PC sem encoder de GPU — `-c copy` não requer
  encode por hardware.
