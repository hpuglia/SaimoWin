# Gravação da saída de vídeo e áudio via OBS (teste)

Quando a gravação direta do HLS retorna **0 bytes**, use a captura do vídeo
já renderizado no Saimo TV com áudio via OBS Studio. Isso não depende da
extensão dos segmentos do servidor e usa codificador de vídeo do sistema.

## Configuração inicial (Windows)

1. Instale/abra [OBS Studio](https://obsproject.com/) (30.1 ou posterior).
2. Crie a cena exatamente chamada **Saimo TV**.
3. Nessa cena, adicione uma **Captura de janela** selecionando a janela
   **Saimo TV [TESTE hpuglia]** (ou Saimo TV para a versão oficial).
4. Marque **Capturar áudio (BETA)** na fonte de Captura de janela. Isso captura
   o som do aplicativo sem exigir Stereo Mix e sem registrar outros programas.
5. Desative o áudio global de Desktop no OBS para não duplicar o som.
6. Em **Configurações > Saída > Gravação** configure pasta local com espaço
   disponível, arquivo **MKV**, codificador **NVIDIA NVENC H.264** (se disponível),
   **CQP 18 a 20**, 1080p/30 fps ou resolução equivalente da fonte. Para
   outros hardwares use codificador adequado. Escolha áudio AAC 192 kb/s.
7. Em **Ferramentas > Configurações do servidor WebSocket**, habilite
   servidor WebSocket na porta 4455, protegida por senha.
8. No Saimo, em **Ajustes > Gravações de TV**, informe o nome da cena
   e a senha OBS. A senha é usada apenas na sessão e não é gravada no disco.
9. No player ao vivo, clique **Gravar > Capturar saída do player com OBS**
   e escolha até parar ou por um tempo determinado. O Saimo testa o estado
   informado pelo OBS e verifica tamanho do arquivo ao finalizar.

### Observações importantes

- O OBS deve permanecer aberto, com a cena correta selecionada. Grava a
  janela e o áudio que ela reproduz. Não é necessário abrir nova conexão HLS.
- A janela capturada precisa continuar sendo renderizada; não minimize o Saimo
  durante a captura, pois o resultado depende do método de captura da plataforma.
- A cena pode incluir controles e avisos exibidos pelo Saimo. Para saída limpa,
  mantenha os controles ocultos e ajuste enquadramento no OBS.
- O destino do OBS é configurado no próprio OBS, diferente do destino do DVR
  direto indicado em Ajustes do Saimo; cada dispositivo define o seu destino.
- O OBS continua sendo um gravador independente: ao encerrar o Saimo,
  confira que a gravação também foi encerrada no OBS.
- A gravação **agendada independente** ainda usa o FFmpeg e não substitui
  automaticamente este motor de captura de janela. Agendar OBS requer que a
  cena permaneça disponível e o Saimo esteja reproduzindo.
- Captura de áudio por aplicativo com Captura de janela está disponível
  no Windows OBS 30.1+. Linux e macOS possuem fontes distintas de
  vídeo/áudio no OBS e precisam de configuração apropriada.
- Conteúdo com DRM ou restrições de captura pode resultar em imagem preta
  ou áudio indisponível. O recurso não contorna essas restrições.

## Qualidade

A cópia direta do fluxo é preferível quando a fonte suporta, pois evita
recompressão. A gravação via OBS com NVENC mantém alta qualidade, mas é
uma nova codificação. Não é correto prometer equivalência bit-a-bit com
a transmissão original.

## Diagnóstico

- **Não conectado:** confira OBS aberto, porta 4455 e WebSocket ativo.
- **Autenticação rejeitada:** confira senha nas configurações WebSocket.
- **Cena incorreta:** selecione a cena de captura configurada.
- **OBS já gravando:** o Saimo não interfere em gravação externa.
- **Arquivo de 0 bytes:** confira fonte de janela/áudio e configurações de saída.
