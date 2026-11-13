# Laurel: guia do teste com amigos

O Laurel guarda os últimos segundos do jogo que você está jogando e salva um clipe quando você aperta uma tecla. Ele acha seus jogos sozinho, grava o som do jogo separado do microfone e deixa tudo no seu PC: nada é enviado para lugar nenhum.

Esta é uma versão de teste. Ela pode ter erros, e é exatamente isso que queremos achar com a sua ajuda.

## O que mudou na 0.1.0-rc.16

Esta versão traz correções que vieram dos logs de vocês e um pedido do dono do projeto.

- **Toque duplo no atalho:** apertar o atalho do clipe duas vezes seguidas salvava dois clipes iguais. Agora, um segundo toque dentro de 1 segundo é ignorado.
- **Teclas que não podem mais ser atalho:** CapsLock, Tab, NumLock, ScrollLock, as teclas de volume, de mídia e de menu, Ctrl+C, Ctrl+V, Ctrl+X, Ctrl+Z, Ctrl+Y, Ctrl+A, Ctrl+S e Ctrl+Alt+Delete. O Laurel não tira a tecla do jogo, então um atalho no CapsLock trocava as maiúsculas a cada clipe. Se você usava uma dessas teclas, o Laurel volta o atalho para o padrão (**F8** para o clipe, **Shift+F8** para a captura de tela), ou deixa a ação sem atalho se o padrão já estiver em uso, e avisa uma vez na tela. Para escolher outro, vá em **Configurações > Atalhos**.
- **Fortnite:** o Laurel agora reconhece o Fortnite mesmo quando não o acha pela biblioteca da Epic.
- **Clipe que não pôde ser salvo:** se você aperta o atalho num jogo em tela cheia e o clipe não pode ser salvo, o Laurel mostra o motivo na tela, mesmo com os avisos sobre o jogo desligados. Por exemplo: o jogo não foi reconhecido (adicione-o em **Configurações > Jogos**) ou a gravação está desligada.
- **Logs:** o Laurel passou a anotar o que achou nas bibliotecas da Steam, da Epic, da Riot e da Xbox. Se algum jogo seu não for reconhecido, **mande os logs** (**Configurações > Avançado > Salvar logs para enviar**) e diga qual jogo e por qual loja você o instalou.

## O que mudou na 0.1.0-rc.15

Esta versão corrige um problema que apareceu nos logs de vocês. Quando um jogo abre a janela e demora alguns segundos para começar a desenhar, como o Fortnite durante o carregamento, o Laurel podia ficar sem conseguir entrar no jogo pelo resto da sessão. Ele passava a gravar pela janela (o aviso **Gravando {jogo} pela janela**), que pesa mais, pode perder quadros e, no Windows 10, deixa a borda amarela em volta do jogo. Agora o Laurel entra no jogo assim que ele começa a desenhar. Ele também passou a anotar nos logs quando está esperando o jogo desenhar, para a gente ver se a correção funcionou no seu PC.

- **Jogue normalmente, inclusive jogos que demoram para carregar, e mande os logs depois** (**Configurações > Avançado > Salvar logs para enviar**). Diga se o aviso **Gravando {jogo} pela janela** ou a borda amarela apareceram em algum jogo.

## O que mudou na 0.1.0-rc.14

Esta versão ajuda a investigar um relato: o vídeo no player do Laurel em câmera lenta depois de mover a janela para um segundo monitor. Não conseguimos repetir o problema, então o Laurel passou a anotar nos logs, a cada trecho que toca no player, em qual monitor ele tocou, a taxa de atualização desse monitor e se o vídeo andou na velocidade certa. Isso não deixa o Laurel mais pesado.

- **Se o vídeo ficar lento no player:** deixe tocar uns 10 segundos, salve os logs logo depois e mande para a gente. Diga também se o som ficou lento junto ou só a imagem.

## O que mudou na 0.1.0-rc.13

Esta versão traz dois pedidos de vocês. Obrigado pelas ideias!

- **Cortar e excluir o original.** No corte, o novo botão **Salvar e excluir o original** salva o trecho cortado no lugar do clipe: o original vai para **Excluídos recentemente**, e o clipe cortado fica com o nome, a data e a marca **Manter** dele. Se mudar de ideia, clique em **Desfazer** no aviso ou restaure o original em **Excluídos recentemente**. O botão **Salvar cópia cortada** continua como antes.
- **Redução de ruído do microfone.** Em **Configurações > Gravação**, na parte **Áudio**, a nova opção **Redução de ruído** tira o ruído de fundo da sua voz, como o de ventilador, teclado e ambiente. Ela vem desligada. O **Speex** é leve; o **RNNoise** dá um resultado melhor, mas usa mais processador enquanto o Laurel grava. A troca vale na hora, para os próximos clipes.

## O que mudou na 0.1.0-rc.12

Esta versão corrige o que o pacote de logs do notebook mostrou. Obrigado a quem mandou!

- **Placa de vídeo reiniciada.** Se o driver da placa de vídeo for instalado, atualizado ou reiniciar com o Laurel aberto, o Laurel reinicia o gravador sozinho em poucos segundos. Antes, ele parava de gravar até ser aberto de novo e podia ocupar cada vez mais memória.
- **Mudar uma configuração com o jogo aberto.** Depois de mudar uma configuração de vídeo, a gravação volta com a imagem do jogo. Antes, às vezes os clipes saíam sem imagem até você desligar e ligar a gravação.
- **Programas que você adicionou.** Um programa que você adicionou à mão, como o Spotify, não tira mais a gravação do jogo numa troca rápida de janela: só se ficar uns 30 segundos na frente. O nome de um programa novo vem do próprio programa, e não do título da janela. Se o nome de algum que você já adicionou ficou estranho (o nome de uma música, por exemplo), clique nele em **Configurações > Jogos**, na parte **Seus jogos**, e troque.
- **Jogos que rodam como administrador.** O aviso continua o mesmo, mas o Laurel para de tentar gravar o jogo enquanto você não o reiniciar como administrador.

## O que mudou na 0.1.0-rc.11

Esta versão é para quem joga Minecraft.

- **Minecraft de qualquer launcher.** O Laurel agora reconhece o Minecraft Java sozinho, venha do launcher oficial ou de launchers de modpacks como o Prism, inclusive modpacks com Fabric, Forge ou NeoForge. Antes era preciso adicionar o Minecraft à mão, e um modpack que usasse outra versão do Java deixava de ser reconhecido. Os clipes saem com o nome **Minecraft**.
- **Se você adicionou o Minecraft à mão,** em **Configurações > Jogos**, na parte **Seus jogos**, pode remover essa entrada: o Laurel já encontra o jogo sem ela.
- **Modpacks pesados.** Durante a tela de carregamento de alguns modpacks, o Laurel pode gravar pela janela do jogo por alguns segundos. Quando o jogo termina de abrir, a gravação volta ao normal sozinha.

## O que mudou na 0.1.0-rc.10

Esta versão corrige o que o último pacote de logs mostrou. Obrigado a quem mandou!

- **Fortnite e outros jogos com anti-cheat.** No Fortnite, o Laurel podia gravar a sessão inteira com a tela do anti-cheat (Easy Anti-Cheat) em vez do jogo: os clipes saíam com o som do jogo, mas só com aquela tela. O Laurel achava que a janela do anti-cheat, que abre antes do jogo, era o próprio jogo. Agora ele ignora essas janelas do Easy Anti-Cheat e do BattlEye e espera o jogo de verdade.
- **A gravação acompanha o jogo.** Se a janela do jogo muda de programa antes de o Laurel conseguir entrar nele, como no caso acima, a gravação agora vai junto para a janela nova, em vez de ficar presa na primeira.
- **Logs mais completos.** Ao fechar o Laurel ou ao atualizar, o log agora diz como o motor de gravação terminou, o que ajuda a separar um fechamento normal de uma falha.

## O que mudou na 0.1.0-rc.9

Esta versão traz as últimas correções dos testes de estabilidade e uma correção para quem usa fone Bluetooth.

- **Fone Bluetooth.** Com o Laurel aberto, um fone Bluetooth com microfone podia ficar no modo de chamada, com o som pior ou até mudo, porque o Laurel deixava o microfone aberto o tempo todo, mesmo sem jogo. Agora o Laurel só abre o microfone enquanto grava um jogo, ou a tela quando a gravação de tela está ligada.
- **Escolher o microfone.** Em **Configurações > Gravação**, a opção nova **Gravar meu microfone** liga e desliga o microfone nos clipes, e **Microfone** deixa escolher qual usar, em vez do **Padrão do Windows**. Com um fone Bluetooth, o modo de chamada ainda acontece enquanto o Laurel grava com o microfone do fone: para evitar, escolha outro microfone ou desligue a opção.
- **Avisos importantes aparecem sempre.** Com **Mostrar avisos sobre os jogos** desligado, que é o padrão, o aviso de que um jogo roda como administrador era só um som. Agora os avisos que querem dizer que o Laurel não está gravando o jogo aparecem sobre o jogo mesmo assim: o jogo que roda como administrador, a gravação pausada com o disco quase cheio e **O Laurel parou de gravar**.
- **Desligar o PC com o Laurel aberto.** Ao desligar ou reiniciar o Windows, o Laurel tentava religar o motor de gravação que o Windows estava fechando. Agora ele espera o Windows terminar.
- **Logs mais úteis.** Se o motor de gravação parar de repente, o log agora diz o motivo e onde aconteceu, o que ajuda a achar a causa. Também sumiram as linhas de erro que apareciam a cada troca da saída de som.

## O que mudou na 0.1.0-rc.8

Esta versão corrige o que o último pacote de logs mostrou. Obrigado a quem mandou!

- **Notebooks com duas placas de vídeo.** Em notebooks com a placa de vídeo do processador e uma placa dedicada (NVIDIA ou AMD), o Laurel gravava na placa mais fraca e não conseguia entrar nos jogos: gravava tudo pela janela, e nos jogos pesados o vídeo saía travado. Agora o Laurel grava na placa dedicada, onde os jogos rodam, e, se um jogo rodar na outra placa, usa um modo de compatibilidade para entrar nele.
- **Microfone bloqueado pelo Windows.** Quando o Windows não deixa os aplicativos da área de trabalho usarem o microfone, os clipes saíam sem a sua voz, sem nenhum aviso. Agora a janela do Laurel mostra o aviso **O Windows está bloqueando o microfone**, com um botão que abre a configuração certa do Windows e outro para tentar de novo.
- **Laurel como administrador.** Com o Laurel aberto como administrador (por exemplo, para o Genshin Impact), um jogo em que ele não conseguia entrar ficava sem imagem. Agora o Laurel passa a gravar a janela do jogo, como nos outros casos.
- **Genshin Impact.** O launcher do Genshin Impact (HoYoPlay) era tratado como o jogo. Agora o Laurel espera o jogo de verdade.

## O que mudou na 0.1.0-rc.7

Esta versão corrige o que os logs dos testes de estabilidade mostraram. Obrigado a quem mandou! Continue jogando normalmente e, depois de algumas sessões, mande os logs de novo.

- **Gravação pela janela só quando precisa.** Quando o Laurel não conseguia entrar no jogo, ele passava a gravar a janela do jogo (o aviso **Gravando {jogo} pela janela**) e ficava assim até você fechar o Laurel. No Windows 10, isso deixa uma **borda amarela** em volta do jogo. Agora o Laurel só conta o tempo para desistir depois de começar a tentar, e, enquanto grava pela janela, continua tentando entrar no jogo. Quando consegue, volta à gravação normal sem parar de gravar, e a borda some.
- **Grava mais cedo.** Ao abrir, o Laurel não espera mais as próprias janelas ficarem prontas para começar a gravar. Em alguns PCs, isso levava até 25 segundos.
- **Jogo minimizado.** Minimizar o jogo por alguns segundos, por exemplo com Alt+Tab em tela cheia, fazia o Laurel achar que o jogo tinha fechado e descartar os segundos já gravados. Agora o jogo só conta como fechado quando fecha de verdade.
- **Nome dos clipes.** Às vezes um clipe ficava com o nome "Laurel" e a data, sem o nome do jogo ou da conquista, porque o arquivo ainda estava em uso logo depois de salvo. Agora o Laurel tenta de novo.
- **Conquistas em mais jogos.** Em jogos que trazem uma versão antiga da biblioteca da Steam, os clipes de conquista não aconteciam. Agora, nesses casos, o Laurel usa a biblioteca da Steam de outro jogo instalado.
- **Corte exato do tamanho certo.** O corte **Exato** saía com o dobro do tamanho do original, ou mais. Agora ele não passa da taxa do clipe original, e o tamanho fica parecido.
- **Excluir durante um corte.** Excluir um clipe enquanto ele é cortado falhava sem aviso. Agora aparece o motivo: o clipe está sendo cortado, e é só tentar de novo depois.
- **Logs menores.** O log do motor de gravação enchia em poucos dias quando o Laurel ficava esperando sem jogo. Isso acabou, e os logs agora dizem Windows 11 em vez de Windows 10 quando é o caso.

## O que mudou na 0.1.0-rc.6

- **Som do jogo nos clipes.** Quando o Laurel não conseguia entrar no jogo logo que ele abria, ele passava a gravar a janela do jogo (com o aviso "Gravando {jogo} pela janela"), mas deixava o som do jogo de fora. Com **Jogo e microfone**, o clipe ficava só com a sua voz; com **Só o jogo**, ficava mudo. Isso acontecia muito no Overwatch. Agora o som do jogo entra também nesse caso. Os clipes que já saíram sem o som do jogo não têm como recuperá-lo.

## O que mudou na 0.1.0-rc.5

Esta versão prepara os **testes de estabilidade**: jogue normalmente e, depois de algumas sessões, mande os logs.

- **Salvar logs para enviar.** Em **Configurações > Avançado**, o botão cria um único `.zip` na pasta **Downloads** e abre a pasta com ele selecionado. É esse arquivo que você manda. Ele traz as cinco últimas sessões do Laurel e o log do motor de gravação, agora com a hora de cada linha, e o seu nome de usuário do Windows fica escondido.
- **Um Laurel por vez.** Abrir o Laurel de novo pelo menu Iniciar, com ele já esperando na bandeja, abria uma segunda cópia que gravava o mesmo jogo outra vez. Agora isso só mostra a janela do Laurel que já está aberto.

## O que mudou na 0.1.0-rc.4

- **Clipes das conquistas da Steam.** Enquanto você joga um jogo da Steam, o Laurel salva um clipe sozinho quando você desbloqueia uma conquista rara: menos de 10% dos jogadores (**Rara**) ou menos de 5% (**Ultra rara**). O clipe sai uns 15 segundos depois da conquista, para mostrar o que veio em seguida, e leva o nome dela, por exemplo `Elsie 2026-10-26 12-48-50 - Hi Mom!.mp4`. Toca um som próprio, três notas subindo.
- **Opções** em **Configurações > Clipes automáticos**: ligar e desligar tudo, o som, as três opções (**Ultra rara**, **Rara** e **Qualquer conquista**, que vem desligada) e uma chave para cada jogo da Steam, para deixar um jogo de fora.
- **Na biblioteca**, os clipes de conquista têm um **troféu azul** na miniatura (passe o mouse para ver a conquista e quantos jogadores a têm), o filtro **Conquistas** na lateral e a linha **Conquista** nos detalhes.
- **Como funciona.** Enquanto o jogo roda, um programa pequeno do Laurel lê as suas conquistas pela própria Steam. Ele não mexe no jogo e fecha junto com ele. A raridade vem da Valve, numa consulta por jogo.

## O que mudou na 0.1.0-rc.3

- **Avisos no jogo desligados por padrão.** Medimos que, enquanto o aviso **Clipe salvo** aparece por cima do jogo, o Windows deixa de mostrar a imagem do jogo pelo caminho mais rápido, e alguns jogos podem ter uma leve instabilidade. Por isso, o aviso visual agora vem desligado, e o som de clipe salvo continua. A atualização desliga o aviso uma vez, mesmo para quem já tinha o Laurel.
- **Avisos mais curtos.** Para quem ligar os avisos em **Configurações > Notificações > Mostrar avisos sobre os jogos**, o aviso de clipe e o de captura de tela agora ficam só 1,5 segundo na tela. Ao ligar, aparece em amarelo o lembrete de que alguns jogos podem ter uma leve instabilidade.

## O que mudou na 0.1.0-rc.2

- **Português.** O Laurel abre no idioma do Windows: português ou inglês. Para trocar, vá em **Configurações > Idioma**.
- **Vários clipes de uma vez.** Na biblioteca, selecione com **Ctrl+clique**, com o **clique direito** ou pela caixinha no canto do clipe; **Shift+clique** pega um intervalo, e **Ctrl+A** pega tudo. A barra que aparece em cima mantém, exclui ou cancela a seleção.
- **Excluídos recentemente.** Excluir não manda mais o clipe para a Lixeira do Windows: ele vai para **Excluídos**, na lateral da biblioteca, e fica lá 7 dias antes de sair de vez. Logo depois de excluir aparece **Desfazer**, e dentro de Excluídos dá para **Restaurar** ou **Excluir agora**. O prazo (1, 7, 15 ou 30 dias) fica em **Configurações > Armazenamento**.
- **Clipes de 30 segundos inteiros.** Antes, alguns clipes saíam com 29 e pouco; agora têm pelo menos o tempo escolhido.
- **Logs mais úteis.** Os logs registram o que pediu cada clipe (o atalho, a bandeja ou o botão), para entendermos casos de clipe salvo sem ninguém apertar o F8.

Quem já tem a versão anterior recebe a atualização pelo próprio Laurel: aparece o aviso de versão nova, e é só clicar em **Instalar e reiniciar** (na 0.1.0-rc.1, que ainda era em inglês, **Install and restart**).

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
2. **Jogo encontrado:** abriu um jogo e o Laurel mostrou **Gravando** com o nome certo? Em jogos com anti-cheat, como o Fortnite, o clipe mostra o jogo, e não a tela do anti-cheat? Se você joga Minecraft, o Laurel mostrou **Gravando** com o nome **Minecraft** sem você adicionar nada? Diga qual launcher e qual modpack você usa. Se você adicionou um programa à mão, ele ainda tira a gravação do jogo quando você troca de janela rapidinho?
3. **Clipe:** o F8 salvou o clipe? Tocou o som de clipe salvo? O clipe tem imagem e som, e pelo menos 30 segundos?
4. **Som:** com o padrão (jogo e microfone), o Discord e a música ficaram fora do clipe? Teste também as outras opções de **Som do clipe**, e **Gravar meu microfone** desligado. Se usa fone Bluetooth com microfone, diga se o som do fone ficou normal com o Laurel aberto, sem jogo, e durante o jogo. Teste a **Redução de ruído** com o **Speex** e com o **RNNoise**: a sua voz ficou limpa e natural no clipe? O jogo ficou mais pesado com o RNNoise? Se aparecer o aviso **Gravando {jogo} pela janela**, confira se o clipe tem o som do jogo e se, depois de um tempo, a gravação voltou ao normal (no Windows 10, a borda amarela some).
5. **Placa de vídeo:** em **Configurações > Gravação**, o item **Gravando agora** mostra o codificador em uso. Diga qual apareceu, qual é a sua placa (NVIDIA, AMD ou Intel) e se o seu PC é um notebook com duas placas de vídeo. Testar em placas AMD e Intel é muito importante.
6. **Desempenho:** sentiu queda de FPS ou travadas, principalmente quando o jogo abriu ou quando salvou um clipe? Se quiser, ligue **Configurações > Notificações > Mostrar avisos sobre os jogos** e diga se sentiu alguma diferença ao salvar com o aviso na tela. Diga também se o seu monitor usa G-Sync ou FreeSync.
7. **Português:** o Laurel abriu em português? Algum texto ficou estranho, cortado ou em inglês? Mande uma captura de tela.
8. **Seleção e Excluídos:** excluir vários clipes de uma vez funcionou? O **Desfazer** trouxe de volta? **Restaurar** devolveu o clipe para a biblioteca?
9. **Corte:** cortar um clipe na biblioteca funcionou? Teste também **Salvar e excluir o original**: o clipe cortado ficou no lugar do original, e o **Desfazer** trouxe o original de volta? Se você tem dois monitores, toque um clipe no player e mova a janela do Laurel para o outro monitor: o vídeo continuou na velocidade normal?
10. **Iniciar com o Windows:** depois de reiniciar o PC, o Laurel abriu sozinho na bandeja e gravou o jogo?
11. **Atualização:** quando sair uma versão de teste nova, o Laurel vai oferecer a atualização sozinho. Diga se funcionou.
12. **Conquistas da Steam:** jogando um jogo da Steam, desbloqueie uma conquista. Se ela for comum, ligue antes **Configurações > Clipes automáticos > Qualquer conquista**. O clipe apareceu uns 15 segundos depois, com o troféu na biblioteca? Tocou o som? Em **Clipes automáticos**, a linha de estado mostrou o jogo e as conquistas? Em jogos com anti-cheat, notou algo diferente? Mande o **app.log** se algo não sair.

## Como relatar um problema

Mande pelo nosso canal de sempre:

- o que você fez, o que esperava e o que aconteceu;
- o jogo, a sua placa de vídeo e a versão do Windows;
- os logs: em **Configurações > Avançado**, clique em **Salvar logs para enviar**. O Laurel cria um único arquivo `.zip` na pasta **Downloads** e abre a pasta com ele selecionado; é só mandar esse arquivo. Ele traz as cinco últimas sessões do Laurel, o log do motor de gravação com a hora de cada linha e as versões do Laurel e do Windows, com o seu nome de usuário do Windows escondido.

## Privacidade

Os clipes e as configurações ficam só no seu PC. O Laurel não tem conta, anúncios nem telemetria. A única coisa que ele busca na internet é se existe uma versão nova, e isso pode ser desligado em **Configurações > Sobre > Verificar atualizações**. A atualização só é instalada quando você clica em **Instalar e reiniciar**.

## Desinstalar

Em **Configurações do Windows > Aplicativos > Aplicativos instalados**, procure **Laurel** e desinstale. Os seus clipes em **Vídeos\Laurel** nunca são apagados, nem os que estiverem em Excluídos. Marque **Remover dados do programa** (no desinstalador em inglês, **Delete the application data**) só se quiser apagar também as configurações e o índice da biblioteca.
