# Prezel

## What is Prezel? 🎯

Prezel is a self-hostable open source Vercel alternative.

Prezel endeavor is giving users the exact same experience they would have in Vercel.
The only difference is that the compute resources needed to run their apps are owned by the user.
That means there is no intermediate extra-charging you for the compute resources you use, you just pay what that is worth,
either if it's your cloud provider bill or your electrycity bill.
In contrast with other open source alternatives to Vercel, the rest of the resources needed, such as DNS records or Github connections, are handled for you.
This means setting your first server up will be a breathe!


## Features 🌟

- Any environment: You can deploy Prezel in any server: DigitalOcean, Linode, Raspberry Pi, EC2, or even your own machine. All it takes is running a single command.
- Framework agnostic: Astro, Next, Laravel. Just choose a repository from your github account, Prezel will do the rest.
- Push to deploy: Updating your production deployment with Prezel is as easy as pushing to the main branch in your Github repository.
- Preview deployments: Every time you open a PR a new deployment will be created to let you and your team discover bugs as soon as posible, with ease!
- Ultra fast and lightweight: Built using Rust, it integrates everything, including the proxy server, in a single tiny binary. Containers for preview deployments are created on demand per user request and removed when idle. All so that the tiniest box won't even notice Prezel is running on it.
- Database branching: If you opt-in in our Sqlite recommendation, you will get database branching for free. A clone of your production DB is made available for each of your previews.
- DB web inspector: You can quickly inspect and edit the data from any of your database branches in a modern web inspector powered by Prisma.
- Free SSL certificates: LetsEncrypt comes built-in with Prezel so you get SSL certificates for all your apps.
- OpenAPI ready: Prezel exposes a REST API right from your server, so you can create custom integrations in your CI/CD pipeline. The only limit is your imagination!
- And so much more... System notifications, system/app logs, free domains per app/deployment, automatic DB backups (coming soon) and the list goes on.

## Installation 🚀

For now, the only officially supported way of installing Prezel is through the main Prezel provider instance.
Just head to prezel.app, create and account, and you will be prompted with the command you need to run on your server.

## License 📜

Prezel is licensed under the GPLv3. You can find the full text of the license in the [LICENSE](./LICENSE) file.
