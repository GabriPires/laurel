# Laurel: guia do teste com amigos

O Laurel guarda os últimos segundos do jogo que você está jogando e salva um clipe quando você aperta uma tecla. Ele acha seus jogos sozinho, grava o som do jogo separado do microfone e deixa tudo no seu PC: nada é enviado para lugar nenhum.

Esta é uma versão de teste. Ela pode ter erros, e é exatamente isso que queremos achar com a sua ajuda.

## O que mudou na 0.1.0-rc.2

- **Português.** O Laurel abre no idioma do Windows: português ou inglês. Para trocar, vá em **Configurações > Idioma**.
- **Vários clipes de uma vez.** Na biblioteca, selecione com **Ctrl+clique**, com o **clique direito** ou pela caixinha no canto do clipe; **Shift+clique** pega um intervalo, e **Ctrl+A** pega tudo. A barra que aparece em cima mantém, exclui ou cancela a seleção.
- **Excluídos recentemente.** Excluir não manda mais o clipe para a Lixeira do Windows: ele vai para **Excluídos**, na lateral da biblioteca, e fica lá 7 dias antes de sair de vez. Logo depois de excluir aparece **Desfazer**, e dentro de Excluídos dá para **Restaurar** ou **Excluir agora**. O prazo (1, 7, 15 ou 30 dias) fica em **Configurações > Armazenamento**.
- **Clipes de 30 segundos inteiros.** Antes, alguns clipes saíam com 29 e pouco; agora têm pelo menos o tempo escolhido.
- **Logs mais úteis.** Os logs registram o que pediu cada clipe (o atalho, a bandeja ou o botão), para entendermos casos de clipe salvo sem ninguém apertar o F8.

Quem já tem a versão anterior recebe a atualização pelo próprio Laurel: aparece o aviso de versão nova, e é só clicar em **Install and restart** (a versão anterior ainda está em inglês).

## Antes de instalar

- Windows 10 (versão 2004 ou mais nova) ou Windows 11, 64 bits.
- Não precisa desinstalar o OBS ou outro gravador de clipes, mas anote quais você tem: isso ajuda a entender o que acontecer.

## Instalar

1. Baixe o `Laurel_..._x64-setup.exe` da página da versão de teste e abra.
2. O instalador ainda não é assinado, então o Windows pode mostrar **"O Windows protegeu o computador"**. Clique em **Mais informações** e depois em **Executar assim mesmo**.
3. O Laurel é instalado só para o seu usuário, sem pedir administrador. A exceção é quando falta no PC o **Microsoft Visual C++ Runtime**, de que o Laurel precisa para gravar: aí o Windows pede permissão para instalá-lo.
4. Na primeira vez que o Laurel abrir, ele avisa que vai iniciar junto com o Windows (**O Laurel inicia com o Windows**). Escolha **Manter ligado** para ele ficar esperando na bandeja, pronto para gravar, ou **Desligar**.

## Usar

- Abra um jogo. O Laurel acha os jogos da Steam, Epic, Riot e Xbox e começa a gravar sozinho; no topo da janela aparece **Gravando** com o nome do jogo.
- **F8** salva um clipe com os últimos 30 segundos. **Shift+F8** tira uma captura de tela. As teclas mudam em **Configurações > Atalhos**.
- Os clipes ficam na pasta **Vídeos\Laurel** e aparecem na biblioteca do Laurel, onde dá para assistir, cortar, manter e excluir.
- Se um jogo não for encontrado, adicione em **Configurações > Jogos > Seus jogos**.
- Se o jogo roda como administrador, o Laurel oferece **Reiniciar como administrador** para conseguir gravá-lo.
- Em **Configurações > Gravação** ficam a duração do clipe, a resolução, o fps, o codificador e o som do clipe (**Som do clipe**: só o jogo; jogo e microfone; ou todo o som do PC e microfone).

## O que testar

Não precisa fazer tudo. O que você conseguir já ajuda muito.

1. **Instalação ou atualização:** instalou ou atualizou sem erro? Pediu administrador? Seu PC tinha OBS instalado?
2. **Jogo encontrado:** abriu um jogo e o Laurel mostrou **Gravando** com o nome certo?
3. **Clipe:** o F8 salvou o clipe? Apareceu o aviso **Clipe salvo** no jogo? O clipe tem imagem e som, e pelo menos 30 segundos?
4. **Som:** com o padrão (jogo e microfone), o Discord e a música ficaram fora do clipe? Teste também as outras opções de **Som do clipe**.
5. **Placa de vídeo:** em **Configurações > Gravação**, o item **Gravando agora** mostra o codificador em uso. Diga qual apareceu e qual é a sua placa (NVIDIA, AMD ou Intel). Testar em placas AMD e Intel é muito importante.
6. **Desempenho:** sentiu queda de FPS ou travadas, principalmente quando o jogo abriu ou quando salvou um clipe? Se sentiu ao salvar, desligue **Configurações > Notificações > Mostrar avisos sobre os jogos**, teste de novo e diga se mudou.
7. **Português:** o Laurel abriu em português? Algum texto ficou estranho, cortado ou em inglês? Mande uma captura de tela.
8. **Seleção e Excluídos:** excluir vários clipes de uma vez funcionou? O **Desfazer** trouxe de volta? **Restaurar** devolveu o clipe para a biblioteca?
9. **Corte:** cortar um clipe na biblioteca funcionou?
10. **Iniciar com o Windows:** depois de reiniciar o PC, o Laurel abriu sozinho na bandeja e gravou o jogo?
11. **Atualização:** quando sair uma versão de teste nova, o Laurel vai oferecer a atualização sozinho. Diga se funcionou.

## Como relatar um problema

Mande pelo nosso canal de sempre:

- o que você fez, o que esperava e o que aconteceu;
- o jogo, a sua placa de vídeo e a versão do Windows;
- os logs: em **Configurações > Avançado**, clique em **Abrir a pasta de logs** e mande os arquivos **app.log** e **engine.log**, e também **app.old.log** e **engine.old.log** se existirem. O app.log recomeça cada vez que o Laurel abre, e a sessão anterior fica no app.old.log.

## Privacidade

Os clipes e as configurações ficam só no seu PC. O Laurel não tem conta, anúncios nem telemetria. A única coisa que ele busca na internet é se existe uma versão nova, e isso pode ser desligado em **Configurações > Sobre > Verificar atualizações**. A atualização só é instalada quando você clica em **Instalar e reiniciar**.

## Desinstalar

Em **Configurações do Windows > Aplicativos > Aplicativos instalados**, procure **Laurel** e desinstale. Os seus clipes em **Vídeos\Laurel** nunca são apagados, nem os que estiverem em Excluídos. Marque **Remover dados do programa** (no desinstalador em inglês, **Delete the application data**) só se quiser apagar também as configurações e o índice da biblioteca.
