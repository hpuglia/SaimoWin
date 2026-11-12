# DVR direto segmentado — SaimoWin V15

O SaimoWin grava a transmissão original com FFmpeg `-c copy`, sem OBS,
captura de tela ou recompressão.

## Pasta organizada automaticamente

A pasta visível contém apenas vídeos finalizados e verificados:

```
Gravacoes/
  SBT SP_2026-11-12_02-54-38.mkv
  _Processando/                     (apenas enquanto necessário)
    Outro Canal_2026-11-12_03-00-00/
      parte_00000.mkv
      parte_00001.mkv
      partes.ffconcat
      sessao.json
      tentativa_1.ffmpeg.log
```

Uma sessão grava primeiro trechos MKV independentes de aproximadamente 30
segundos, conforme os keyframes da fonte. Depois de interromper ou atingir o
prazo, o Saimo reúne os trechos, verifica a presença de vídeo **e áudio** com
ffprobe e publica um MKV com nome exclusivo na pasta `Gravacoes`.

**Somente após validação e publicação**, os arquivos temporários conhecidos
são apagados. Arquivos estranhos, como notas colocadas pelo usuário, não são
apagados pelo limpador. Se faltar ffprobe, houver falha na união, vídeo
inválido ou ausência de áudio, o programa **preserva os arquivos e logs** em
`_Processando` para recuperação e registra o erro. Nenhum arquivo final
existente é sobrescrito.

Ao abrir a V15, gravações bem-sucedidas deixadas na estrutura da V14 são
validadas, movidas para a pasta principal e limpas da mesma maneira.

## Estados no player

O botão circular de gravação fica acima do selo "AO VIVO" e mostra:
- PARADO — anel cinza, sem processo ativo naquele canal;
- PREPARANDO — anel amarelo, FFmpeg ainda não confirmou dados;
- GRAVANDO — anel vermelho pulsante após detectar dados reais;
- PROCESSANDO — anel azul durante encerramento e união;
- SALVO — anel verde após publicação validada;
- ERRO — anel vermelho-claro e aviso curto quando falha.

Os avisos do aplicativo complementam o indicador, com detalhes de falhas
guardados nos arquivos de diagnóstico em vez de ocupar a tela.

## Fontes especiais

Fontes HLS com URL `.txt` usam o formato forçado `-f hls`, como o mpv.
Se a primeira tentativa falhar sem bytes, há um segundo modo restrito de
extensões (incluindo `.woff`), sem desativar globalmente os filtros FFmpeg.
Não contorna DRM nem servidores que rejeitem ou expirem URLs.

## Recuperação e agendamento

Na próxima inicialização, o DVR procura sessões interrompidas e tenta
remontar trechos existentes. É possível perder o segmento incompleto numa
queda de energia; **100% de recuperação não pode ser garantida**.

Agendamentos usam o mesmo gravador direto, mas precisam que o computador
esteja ligado e o Saimo aberto no horário programado. Não exigem que o canal
esteja visível durante a gravação. Um agente residente para iniciar com o
aplicativo fechado é trabalho futuro.

O destino é por dispositivo, configurável em Ajustes; no teste em Windows
utilizamos `SAIMO_GRAVACOES_DIR` para direcionar os arquivos ao disco J: ou E:.
É necessário disponibilizar `ffmpeg` e `ffprobe` portáteis junto do
executável para gravação e manutenção completas.
