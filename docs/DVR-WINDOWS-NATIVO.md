# Saimo DVR Windows — especificação da captura nativa (prova de conceito)

Status: **ARQUITETURA E POLÍTICA DE FALLBACK; NÃO IMPLEMENTAÇÃO DO GRAVADOR NATIVO AINDA**.

A experiência de OBS exigindo configuração externa não atende ao requisito de software
portátil e leve. Em Windows usaremos APIs integradas ao sistema operacional,
sem obrigar o usuário a instalar OBS, FFmpeg ou driver virtual.

## Motor escolhido

Adotar e adaptar a arquitetura comprovada pelo projeto de domínio público
[wcap](https://github.com/mmozeiko/wcap) (licença Unlicense):

- Windows.Graphics.Capture (WGC): quadros da janela ativa, sem copiar via CPU;
- WASAPI process loopback: áudio somente do Saimo, quando o Windows suporta;
- Media Foundation: H.264 por hardware (Intel/NVIDIA/AMD) com alternativa de software;
- AAC estéreo, com áudio sincronizado por carimbo de tempo;
- **MP4 fragmentado**: dados já escritos tendem a sobreviver sem finalização do arquivo.
- Resolução e FPS adaptados: 1080p/30 para hardware capaz, 720p/24-30 para PC fraco.
- Capturar somente a janela de vídeo. Não desligar DRM, não capturar telas protegidas.

Isso pode ser implementado como biblioteca adaptada integrada ao binário ou
subprocesso portátil de poucos MB, com controle explícito Start/Stop por IPC
e retorno do caminho do arquivo. **Não usar automação de teclas globais** na
integração definitiva; no protótipo independente a interação permanece manual.

## Política de fallback

1. Para URLs diretas HLS/TS usuais, testar cópia de pacotes sem recompressão;
   só declarar sucesso quando o arquivo tiver crescido e houver áudio/vídeo válidos.
2. Para fontes como `.woff`, `.txt` ou reprodução direta mal sucedida,
   escolher WGC+WASAPI+Media Foundation, gravando o que é realmente reproduzido.
3. Detectar encoder H.264 por hardware. Se indisponível/falhar, tentar H.264
   software automaticamente reduzindo resolução/FPS antes de comprometer a reprodução.
4. Windows 10 build 20348+: usar áudio do processo. Em builds anteriores,
   oferecer captura de áudio do sistema **somente com consentimento explícito**,
   ou informar que áudio não estará disponível.
5. Fontes protegidas e captura bloqueada por DRM: erro claro, sem contorno.
6. Caso capture window falhe e usuário autorize, usar Desktop Duplication; pode
   incluir outras janelas, então sempre sinalizar o alcance na interface.

Em caso de erro de gravação, NUNCA informar apenas "gravação iniciada".
Usar estado: iniciando -> verificando dados -> gravando -> finalizando -> concluída/erro.
Afirmar que áudio existe somente quando for realmente verificado.

## Quedas e recuperação

- Prefira MP4 fragmentado com flush periódico e manifeste o arquivo como
  `incompleto` enquanto o job estiver ativo.
- Manter `session.json` gravado atomicamente, com horário, canal, progresso,
  bytes no disco, método, codec e último segmento consistente.
- Ao reiniciar, procurar sessões não finalizadas e verificar o que pode ser
  recuperado; NUNCA apagar clipes parciais porque não estão completos.
- Se usar segmentos independentes, não sobrescrever arquivos anteriores;
  permitir leitura individual e recomposição posterior, sem assumir continuidade
  perfeita entre reinicializações.
- Falta de espaço: interromper com erro, preservando o que foi escrito.
- Após queda de energia, falha física de disco ou falta de dados em memória,
  **não é tecnicamente possível garantir 100% dos últimos quadros**.

## Agendamento

- Para PC ligado com Saimo aberto: job persistente dispara a gravação na
  hora configurada. A captura de janela requer que um player esteja renderizando;
  o agendamento deve preparar a janela/fonte antes do início.
- Para gravação com app fechado: precisa de agente residente/scheduler OS;
  não afirmar que isso funciona apenas salvando uma data no JSON.
- Os trabalhos agendados podem usar cópia direta em background sempre que
  compatível e, caso contrário, captura nativa do player.

## Testes de aceitação para integrar ao ramo principal

- PC sem NVENC e CPU modesta, 720p/24-30 AAC: sem congelar a reprodução.
- RTX 3060 1080p/30 AAC: áudio sincronizado, sem 0 bytes.
- HLS comum e HLS disfarçado: arquivos gerados e reproduzíveis.
- Alternar canal durante gravação: encerra uma sessão antes de trocar.
- Interromper processo na metade: trecho anterior ao encerramento permanece
  reproduzível; registrar limite temporal da perda.
- Usuário nega captura de áudio geral: não gravar outros programas.
- Dispositivo sem espaço: aviso claro, sem apagar gravações anteriores.
- Testes automatizados do seletor em `src/dvr_politica.rs` passam em CI.
- Captura de conteúdo DRM não é contornada.

### Referências

- https://github.com/mmozeiko/wcap (Unlicense)
- https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture
- https://github.com/microsoft/Windows-classic-samples/tree/main/Samples/ApplicationLoopback
- https://learn.microsoft.com/en-us/windows/win32/medfound/media-foundation-sdk
